//! The opcode to [`ServerPacket`] dispatch, one match arm per opcode.

use std::io::{self, Read};

use crate::wire::{
    capacity_hint, read_cstring, read_f32_le, read_packed_guid, read_u16_le, read_u32_le,
    read_u64_le, read_u8, Vector3d,
};

use super::{
    action_bar, area_trigger, attack, auction, bank, battlefield, binder, broadcast, channel, chat,
    combat_log, death, duel, gameobject, gm_ticket, gossip, group, guild, instance, items, loot,
    mail, meeting_stone, mirror_timer, monster_move, movement, opcode, page_text, pet, petition,
    pose, progression, pvp, quest, social, spellbook, spells, stable, summon, tabard, taxi, trade,
    trainer, tutorial, update_object, vendor, world_state, AttackSwingError, Character,
    CreatureQueryInfo, JumpInfo, MoveMode, ServerPacket, SpeedKind, SplineMode,
};

/// Read a `SMSG_FORCE_*_SPEED_CHANGE` body: `[packed guid][u32 counter][f32 speed]` for all six
/// kinds (vmangos `SendSpeedChangeToController`, the `> 1_9_4` branch).
fn read_force_speed(kind: SpeedKind, r: &mut impl Read) -> io::Result<ServerPacket> {
    Ok(ServerPacket::ForceSpeedChange {
        guid: read_packed_guid(r)?,
        kind,
        counter: read_u32_le(r)?,
        speed: read_f32_le(r)?,
    })
}

/// Read an `SMSG_COMPRESSED_MOVES` body: `u32` uncompressed size, then deflated
/// `[u8 size][u16 opcode][body]` records, `size` counting the opcode (`MovementData::AddPacket`).
/// Each record goes back through [`parse_server`], and one bad record fails the whole batch; only
/// the `MSG_MOVE_*` relays can appear (vmangos `ObjectViewersMovementDeliverer`).
fn read_compressed_moves(r: &mut &[u8]) -> io::Result<Vec<ServerPacket>> {
    let uncompressed = read_u32_le(r)? as usize;
    let mut buf = Vec::with_capacity(uncompressed.min(64 * 1024));
    // `bufread`, not `read`: the cursor must stop at the stream's end for the tail to be seen.
    flate2::bufread::ZlibDecoder::new(r).read_to_end(&mut buf)?;
    let mut rest = buf.as_slice();
    let mut packets = Vec::new();
    while !rest.is_empty() {
        let size = read_u8(&mut rest)? as usize;
        let opcode = read_u16_le(&mut rest)?;
        // `size` spans opcode and body, so under 2 means the stream is misframed.
        let body_len = size.checked_sub(2).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("compressed-moves record size {size} < 2"),
            )
        })?;
        if rest.len() < body_len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "compressed-moves record {opcode:#06x} wants {body_len}B, {}B left",
                    rest.len()
                ),
            ));
        }
        let (body, tail) = rest.split_at(body_len);
        rest = tail;
        // A batch inside a batch would recurse without bound; vmangos never nests one.
        if opcode == opcode::SMSG_COMPRESSED_MOVES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "compressed-moves nested inside compressed-moves",
            ));
        }
        packets.push(parse_server(opcode, body).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "compressed-moves record {opcode:#06x} ({}): {e}",
                    super::opcode_name(opcode).unwrap_or("?")
                ),
            )
        })?);
    }
    Ok(packets)
}

/// Read a `SMSG_SPLINE_SET_*_SPEED` body: `[packed guid][f32 speed]`, no counter, no ack
/// (vmangos `SendSpeedChangeToAll`, the `> 1_8_4` layout).
fn read_spline_speed(kind: SpeedKind, r: &mut impl Read) -> io::Result<ServerPacket> {
    Ok(ServerPacket::SplineSpeedChange {
        guid: read_packed_guid(r)?,
        kind,
        speed: read_f32_le(r)?,
    })
}

/// Read a `MSG_MOVE_SET_*_SPEED` body: `[packed guid][MovementInfo][f32 speed]`, no ack
/// (vmangos `SendSpeedChangeToObservers`).
fn read_move_set_speed(kind: SpeedKind, r: &mut impl Read) -> io::Result<ServerPacket> {
    let guid = read_packed_guid(r)?;
    let info = movement::read_movement_info(r)?;
    Ok(ServerPacket::MoveSetSpeed {
        guid,
        kind,
        flags: info.flags,
        position: info.position,
        orientation: info.orientation,
        pitch: info.pitch,
        time: info.timestamp,
        fall_time: info.fall_time,
        jump: info.jump,
        transport: info.transport,
        speed: read_f32_le(r)?,
    })
}

/// True for an opcode the server relays as `[packed guid][MovementInfo]`, one client handler
/// (`0x603bb0`): the echoed input stream (vmangos `HandleMovementOpcodes`), and the observer leg
/// of root, hover, feather-fall, water-walk and teleport, with no ack and its direction in the
/// flags word. `MSG_MOVE_TELEPORT` (197) has no counter, unlike `MSG_MOVE_TELEPORT_ACK` (199).
const fn is_movement_relay(o: u16) -> bool {
    matches!(
        o,
        opcode::MSG_MOVE_START_FORWARD
            | opcode::MSG_MOVE_START_BACKWARD
            | opcode::MSG_MOVE_STOP
            | opcode::MSG_MOVE_START_STRAFE_LEFT
            | opcode::MSG_MOVE_START_STRAFE_RIGHT
            | opcode::MSG_MOVE_STOP_STRAFE
            | opcode::MSG_MOVE_JUMP
            | opcode::MSG_MOVE_START_TURN_LEFT
            | opcode::MSG_MOVE_START_TURN_RIGHT
            | opcode::MSG_MOVE_STOP_TURN
            | opcode::MSG_MOVE_START_PITCH_UP
            | opcode::MSG_MOVE_START_PITCH_DOWN
            | opcode::MSG_MOVE_STOP_PITCH
            | opcode::MSG_MOVE_SET_RUN_MODE
            | opcode::MSG_MOVE_SET_WALK_MODE
            | opcode::MSG_MOVE_FALL_LAND
            | opcode::MSG_MOVE_START_SWIM
            | opcode::MSG_MOVE_STOP_SWIM
            | opcode::MSG_MOVE_SET_FACING
            | opcode::MSG_MOVE_SET_PITCH
            | opcode::MSG_MOVE_HEARTBEAT
            // Another unit's knockback: the four trailing launch floats go unread, since the
            // `MovementInfo` comes from the victim's ack and its jump tail already is that quad.
            | opcode::MSG_MOVE_KNOCK_BACK
            // Another unit's mode change: the mode rides the `MovementInfo` flags word.
            | opcode::MSG_MOVE_ROOT
            | opcode::MSG_MOVE_UNROOT
            | opcode::MSG_MOVE_HOVER
            | opcode::MSG_MOVE_FEATHER_FALL
            | opcode::MSG_MOVE_WATER_WALK
            | opcode::MSG_MOVE_TELEPORT
    )
}

/// `SMSG_ADDON_INFO`'s records, with no count, names or trailer, as `0x51da70` reads each:
///
/// ```text
/// u8  status              ; 2 -> [rec+0x29] = 1 (hidden from the Lua index space)
///                         ; 0 -> [rec+0x24] = 2 (rejected)
///                         ; else -> verify the .toc/Bindings.xml signature
/// u8  infoProvided        ; persisted into the .pub and echoed next logon
/// if infoProvided:
///     u8 keyProvided
///     if keyProvided: u8[256] modulus
///     u32 revision
/// u8  urlProvided
/// if urlProvided: u8[256] url
/// ```
///
/// Retail sends the minimal form, 12 records of `{2, 1, 0, 0u32, 0}`. A truncated record ends the
/// walk and keeps the whole records before it, so `statuses[i]` is always record i's.
fn read_addon_info(r: &mut &[u8]) -> Vec<u8> {
    let mut statuses = Vec::new();
    while !r.is_empty() {
        let Ok(status) = read_u8(r) else { break };
        let Ok(info_provided) = read_u8(r) else { break };
        if info_provided != 0 {
            let Ok(key_provided) = read_u8(r) else { break };
            if key_provided != 0 && skip(r, 256).is_none() {
                break;
            }
            if read_u32_le(r).is_err() {
                break;
            }
        }
        let Ok(url_provided) = read_u8(r) else { break };
        if url_provided != 0 && skip(r, 256).is_none() {
            break;
        }
        statuses.push(status);
    }
    statuses
}

fn skip(r: &mut &[u8], n: usize) -> Option<()> {
    (r.len() >= n).then(|| *r = &r[n..])
}

/// Decode one server packet body into its [`ServerPacket`], ignoring any unread tail that
/// [`parse_server_with_tail`] would report.
pub fn parse_server(opcode: u16, body: &[u8]) -> io::Result<ServerPacket> {
    parse_server_with_tail(opcode, body).map(|(packet, _)| packet)
}

/// [`parse_server`], plus how many body bytes the decoder left unread: a count, never a failure,
/// that exposes a decoder shorter than the server's layout. [`ServerPacket::Other`] reports `0`.
pub fn parse_server_with_tail(opcode: u16, body: &[u8]) -> io::Result<(ServerPacket, usize)> {
    let mut r = body;
    // The inflated leftover of the compressed update object, the one arm with a second stream.
    let mut inner_tail = 0;
    let packet = parse_server_body(opcode, &mut r, &mut inner_tail)?;
    let tail = match packet {
        ServerPacket::Other { .. } => 0,
        _ => r.len() + inner_tail,
    };
    Ok((packet, tail))
}

/// The opcode dispatch; `cursor` advances past what the arm read only on success.
fn parse_server_body(
    opcode: u16,
    cursor: &mut &[u8],
    inner_tail: &mut usize,
) -> io::Result<ServerPacket> {
    let mut r: &[u8] = cursor;
    let packet = match opcode {
        opcode::SMSG_AUTH_CHALLENGE => ServerPacket::AuthChallenge {
            server_seed: read_u32_le(&mut r)?,
        },
        opcode::SMSG_AUTH_RESPONSE => {
            // The client's grammar (`0x5b41b0`): `u8 code`; then, for OK or WAIT_QUEUE with 5+
            // bytes left, `u32 billingTimeRemaining, u8 billingPlanFlags, u32 billingTimeRested`;
            // then, for WAIT_QUEUE, `u32 position`. The 5 guarding a 9-byte group is the client's
            // and mis-parses a 6..=9 byte body; matched on purpose. Deviation: a body too short
            // for the position gives `None` where the client redisplays a stale global, because
            // showing a stale queue position as current is a client bug.
            let result = read_u8(&mut r)?;
            let queued = result == super::AUTH_WAIT_QUEUE;
            let mut billing_time_rested = None;
            if (result == super::AUTH_OK || queued) && r.len() >= 5 {
                let _billing_time_remaining = read_u32_le(&mut r)?;
                let _billing_plan_flags = read_u8(&mut r)?;
                // The only source of `GetBillingTimeRested()` (the client's writer is `0x5b421a`).
                billing_time_rested = Some(read_u32_le(&mut r)?);
            }
            ServerPacket::AuthResponse {
                result,
                queue_position: queued.then(|| read_u32_le(&mut r).ok()).flatten(),
                billing_time_rested,
            }
        }
        opcode::SMSG_CHAR_ENUM => {
            let count = read_u8(&mut r)?;
            // vmangos clamps `CharactersPerRealm` to 10 (`World.cpp:629`).
            let mut characters = Vec::with_capacity(capacity_hint(count, 10));
            for _ in 0..count {
                characters.push(Character::read(&mut r)?);
            }
            ServerPacket::CharEnum { characters }
        }
        opcode::SMSG_CHAR_DELETE => ServerPacket::CharDelete {
            result: read_u8(&mut r)?,
        },
        opcode::SMSG_CHAR_CREATE => ServerPacket::CharCreate {
            result: read_u8(&mut r)?,
        },
        opcode::SMSG_CHARACTER_LOGIN_FAILED => ServerPacket::CharacterLoginFailed {
            result: read_u8(&mut r)?,
        },
        opcode::SMSG_UPDATE_OBJECT => ServerPacket::UpdateObject {
            objects: update_object::read_update_object(&mut r)?,
        },
        opcode::SMSG_DESTROY_OBJECT => ServerPacket::DestroyObject {
            guid: read_u64_le(&mut r)?,
        },
        opcode::SMSG_TRIGGER_CINEMATIC => ServerPacket::TriggerCinematic {
            cinematic_id: read_u32_le(&mut r)?,
        },
        opcode::SMSG_COMPRESSED_UPDATE_OBJECT => {
            let _decompressed_size = read_u32_le(&mut r)?;
            // `&mut r` so the cursor advances over the zlib bytes (by value it reads a copy), and
            // `bufread`, not `read`, whose 32 KiB buffer would swallow bytes after the stream.
            let mut decoder = flate2::bufread::ZlibDecoder::new(&mut r);
            let mut decompressed = Vec::new();
            decoder.read_to_end(&mut decompressed)?;
            drop(decoder);
            let mut dr = decompressed.as_slice();
            let objects = update_object::read_update_object(&mut dr)?;
            *inner_tail = dr.len();
            ServerPacket::UpdateObject { objects }
        }
        opcode::SMSG_COMPRESSED_MOVES => ServerPacket::CompressedMoves {
            packets: read_compressed_moves(&mut r)?,
        },
        opcode::MSG_MOVE_TIME_SKIPPED => {
            let (guid, lag_ms) = movement::read_move_time_skipped(&mut r)?;
            ServerPacket::MoveTimeSkipped { guid, lag_ms }
        }
        opcode::SMSG_MONSTER_MOVE => monster_move::read_monster_move(&mut r, false)?,
        opcode::SMSG_MONSTER_MOVE_TRANSPORT => monster_move::read_monster_move(&mut r, true)?,
        opcode::MSG_MOVE_TELEPORT_ACK => {
            let guid = read_packed_guid(&mut r)?;
            let counter = read_u32_le(&mut r)?;
            let info = movement::read_movement_info(&mut r)?;
            ServerPacket::Teleport {
                guid,
                counter,
                position: info.position,
                orientation: info.orientation,
            }
        }
        opcode::SMSG_NEW_WORLD => {
            let map = read_u32_le(&mut r)?;
            let position = Vector3d::read(&mut r)?;
            let orientation = read_f32_le(&mut r)?;
            ServerPacket::NewWorld {
                map,
                position,
                orientation,
            }
        }
        opcode::SMSG_LOGIN_VERIFY_WORLD => {
            let map = read_u32_le(&mut r)?;
            let position = Vector3d::read(&mut r)?;
            let orientation = read_f32_le(&mut r)?;
            ServerPacket::LoginVerifyWorld {
                map,
                position,
                orientation,
            }
        }
        opcode::SMSG_TRANSFER_PENDING => {
            // `u32 newMapId`, then `{u32 transportEntry, u32 oldMapId}` only when riding a
            // transport (`Misc.cpp:493-501`), which makes the following NEW_WORLD boat-local.
            let map = read_u32_le(&mut r)?;
            let transport = if r.is_empty() {
                None
            } else {
                let entry = read_u32_le(&mut r)?;
                let old_map = read_u32_le(&mut r)?;
                Some((entry, old_map))
            };
            ServerPacket::TransferPending { map, transport }
        }
        opcode::SMSG_TRANSFER_ABORTED => ServerPacket::TransferAborted {
            reason: read_u8(&mut r)?,
        },
        opcode::SMSG_LOGIN_SETTIMESPEED => {
            // Packed DateTime, LSB up: minute:6, hour:5, weekday:3, day:6, month:4, year:5. The
            // day serial assumes 31-day months and 372-day years, monotonic but not calendar-true.
            let datetime = read_u32_le(&mut r)?;
            let timescale = read_f32_le(&mut r)?;
            let (day, month, year) = (
                (datetime >> 14) & 0x3F,
                (datetime >> 20) & 0x0F,
                (datetime >> 24) & 0x1F,
            );
            ServerPacket::TimeSpeed {
                hours: ((datetime >> 6) & 0x1F) as u8,
                minutes: (datetime & 0x3F) as u8,
                day_serial: year * 372 + month * 31 + day,
                timescale,
            }
        }
        // Wall-clock unix seconds (`QueryHandler.cpp:418-423`), not the game clock above.
        opcode::SMSG_QUERY_TIME_RESPONSE => ServerPacket::QueryTimeResponse {
            unix_time: read_u32_le(&mut r)?,
        },
        opcode::SMSG_BINDPOINTUPDATE => {
            // vmangos BindpointUpdate::AppendBodyTo (Packets/Misc.cpp): x, y, z, mapId, areaId.
            let position = Vector3d::read(&mut r)?;
            let map = read_u32_le(&mut r)?;
            let area = read_u32_le(&mut r)?;
            ServerPacket::BindPoint {
                position,
                map,
                area,
            }
        }
        // The ticket response opcodes share one 4-byte body; only the opcode names the verb.
        opcode::SMSG_GMTICKET_GETTICKET => ServerPacket::GmTicketAnswer {
            ticket: gm_ticket::read_gm_ticket(&mut r)?.map(Box::new),
        },
        opcode::SMSG_GMTICKET_CREATE => ServerPacket::GmTicketCreated {
            response: gm_ticket::read_gm_ticket_response(&mut r)?,
        },
        opcode::SMSG_GMTICKET_UPDATETEXT => ServerPacket::GmTicketUpdated {
            response: gm_ticket::read_gm_ticket_response(&mut r)?,
        },
        opcode::SMSG_GMTICKET_DELETETICKET => ServerPacket::GmTicketDeleted {
            response: gm_ticket::read_gm_ticket_response(&mut r)?,
        },
        opcode::SMSG_GMTICKET_SYSTEMSTATUS => ServerPacket::GmTicketSystemStatus {
            status: gm_ticket::read_gm_ticket_system_status(&mut r)?,
        },
        opcode::SMSG_GM_TICKET_STATUS_UPDATE => ServerPacket::GmTicketStatusUpdate {
            status: gm_ticket::read_gm_ticket_response(&mut r)?,
        },
        opcode::SMSG_BINDER_CONFIRM => ServerPacket::BinderConfirm {
            binder: binder::read_binder_confirm(&mut r)?,
        },
        // The summon question; the client's whole answer is the guid back.
        opcode::SMSG_SUMMON_REQUEST => {
            let ask = summon::read_summon_request(&mut r)?;
            ServerPacket::SummonRequest {
                summoner: ask.summoner,
                zone: ask.zone,
                delay_ms: ask.delay_ms,
            }
        }
        // Two-way `MSG_`: inbound is the question (guid, cost); our answer is the guid alone.
        opcode::MSG_TALENT_WIPE_CONFIRM => {
            let ask = progression::read_talent_wipe_confirm(&mut r)?;
            ServerPacket::TalentWipeConfirm {
                trainer: ask.trainer,
                cost: ask.cost,
            }
        }
        opcode::SMSG_PET_UNLEARN_CONFIRM => {
            let ask = pet::read_pet_unlearn_confirm(&mut r)?;
            ServerPacket::PetUnlearnConfirm {
                trainer: ask.trainer,
                cost: ask.cost,
            }
        }
        opcode::SMSG_RAID_GROUP_ONLY => {
            let boot = instance::read_raid_group_only(&mut r)?;
            ServerPacket::RaidGroupOnly {
                delay_ms: boot.delay_ms,
                reason: boot.reason,
            }
        }
        opcode::SMSG_AREA_SPIRIT_HEALER_TIME => {
            let t = death::read_area_spirit_healer_time(&mut r)?;
            ServerPacket::AreaSpiritHealerTime {
                healer: t.healer,
                ms: t.ms,
            }
        }
        opcode::SMSG_BATTLEFIELD_STATUS => {
            ServerPacket::BattlefieldStatus(battlefield::read_battlefield_status(&mut r)?)
        }
        opcode::MSG_PVP_LOG_DATA => {
            ServerPacket::PvpLogData(battlefield::read_pvp_log_data(&mut r)?)
        }
        opcode::SMSG_BATTLEFIELD_LIST => {
            ServerPacket::BattlefieldList(battlefield::read_battlefield_list(&mut r)?)
        }
        opcode::MSG_BATTLEGROUND_PLAYER_POSITIONS => {
            ServerPacket::BattlefieldPositions(battlefield::read_battlefield_positions(&mut r)?)
        }
        opcode::MSG_TABARDVENDOR_ACTIVATE => {
            ServerPacket::TabardVendorActivate(tabard::read_tabard_vendor_activate(&mut r)?)
        }
        opcode::MSG_SAVE_GUILD_EMBLEM => {
            ServerPacket::SaveGuildEmblemResult(tabard::read_save_guild_emblem_result(&mut r)?)
        }
        opcode::SMSG_GROUP_JOINED_BATTLEGROUND => ServerPacket::GroupJoinedBattleground {
            result: crate::wire::read_u32_le(&mut r)?,
        },
        opcode::SMSG_BATTLEGROUND_PLAYER_JOINED => ServerPacket::BattlegroundPlayer {
            guid: crate::wire::read_u64_le(&mut r)?,
            joined: true,
        },
        opcode::SMSG_BATTLEGROUND_PLAYER_LEFT => ServerPacket::BattlegroundPlayer {
            guid: crate::wire::read_u64_le(&mut r)?,
            joined: false,
        },
        opcode::SMSG_MEETINGSTONE_SETQUEUE => {
            let q = meeting_stone::read_meeting_stone_set_queue(&mut r)?;
            ServerPacket::MeetingStoneSetQueue {
                area: q.area,
                status: q.status,
            }
        }
        opcode::SMSG_MEETINGSTONE_SUCCESS => {
            ServerPacket::MeetingStoneNotice(crate::messages::MeetingStoneNotice::Success)
        }
        opcode::SMSG_MEETINGSTONE_IN_PROGRESS => {
            ServerPacket::MeetingStoneNotice(crate::messages::MeetingStoneNotice::InProgress)
        }
        opcode::SMSG_MEETINGSTONE_MEMBER_ADDED => ServerPacket::MeetingStoneNotice(
            meeting_stone::read_meeting_stone_member_added(&mut r)?,
        ),
        opcode::SMSG_MEETINGSTONE_JOIN_FAILED => {
            ServerPacket::MeetingStoneNotice(meeting_stone::read_meeting_stone_join_failed(&mut r)?)
        }
        opcode::SMSG_TUTORIAL_FLAGS => {
            ServerPacket::TutorialFlags(tutorial::read_tutorial_flags(&mut r)?)
        }
        opcode::SMSG_PLAYERBOUND => {
            let bound = binder::read_player_bound(&mut r)?;
            ServerPacket::PlayerBound {
                binder: bound.binder,
                area: bound.area,
            }
        }
        opcode::SMSG_SET_PROFICIENCY => {
            // vmangos SetProficiency::AppendBodyTo (Packets/Skill.cpp): u8 itemClass + u32 mask.
            let item_class = read_u8(&mut r)?;
            let subclass_mask = read_u32_le(&mut r)?;
            ServerPacket::SetProficiency {
                item_class,
                subclass_mask,
            }
        }
        opcode::SMSG_PLAY_SOUND => ServerPacket::PlaySound {
            sound_id: read_u32_le(&mut r)?,
        },
        opcode::SMSG_PLAY_MUSIC => ServerPacket::PlayMusic {
            music_id: read_u32_le(&mut r)?,
        },
        opcode::SMSG_PLAY_OBJECT_SOUND => ServerPacket::PlayObjectSound {
            sound_id: read_u32_le(&mut r)?,
            guid: read_u64_le(&mut r)?,
        },
        opcode::SMSG_WEATHER => ServerPacket::Weather {
            weather_type: read_u32_le(&mut r)?,
            grade: read_f32_le(&mut r)?,
            sound_id: read_u32_le(&mut r)?,
            instant: read_u8(&mut r)? != 0,
        },
        opcode::SMSG_TEXT_EMOTE => {
            let guid = read_u64_le(&mut r)?;
            let text_emote = read_u32_le(&mut r)?;
            // `emoteNum` picks neither sentence nor voice kit on the reference's receive side.
            let _emote_num = read_u32_le(&mut r)?;
            // The target's name, length-prefixed including its NUL (a lone `0x00` for no
            // target); trimmed at the first NUL, so no target reads as "", the untargeted form.
            let namelen = read_u32_le(&mut r)? as usize;
            let mut name = Vec::with_capacity(capacity_hint(namelen, 64));
            for _ in 0..namelen {
                name.push(read_u8(&mut r)?);
            }
            let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            let target_name = String::from_utf8_lossy(&name[..end]).into_owned();
            ServerPacket::TextEmote {
                guid,
                text_emote,
                target_name,
            }
        }
        opcode::SMSG_EMOTE => {
            let emote_id = read_u32_le(&mut r)?;
            let guid = read_u64_le(&mut r)?;
            ServerPacket::Emote { guid, emote_id }
        }
        opcode::SMSG_ITEM_QUERY_SINGLE_RESPONSE => {
            let (entry, info) = items::read_item_query_response(&mut r)?;
            ServerPacket::ItemQueryResponse {
                entry,
                info: info.map(Box::new),
            }
        }
        opcode::SMSG_MESSAGECHAT => ServerPacket::MessageChat(chat::read_message_chat(&mut r)?),
        opcode::SMSG_CHANNEL_NOTIFY => {
            ServerPacket::ChannelNotify(channel::read_channel_notify(&mut r)?)
        }
        opcode::SMSG_CHANNEL_LIST => {
            let (name, flags, members) = channel::read_channel_list(&mut r)?;
            ServerPacket::ChannelList {
                channel: name,
                flags,
                members,
            }
        }
        opcode::SMSG_CHAT_PLAYER_NOT_FOUND => ServerPacket::ChatPlayerNotFound {
            name: chat::read_chat_player_not_found(&mut r)?,
        },
        opcode::SMSG_CHAT_WRONG_FACTION => ServerPacket::ChatWrongFaction,
        opcode::SMSG_NOTIFICATION => ServerPacket::Notification {
            text: chat::read_notification(&mut r)?,
        },
        opcode::SMSG_AREA_TRIGGER_MESSAGE => ServerPacket::AreaTriggerMessage {
            text: area_trigger::read_area_trigger_message(&mut r)?,
        },
        opcode::SMSG_SERVER_MESSAGE => {
            let (message_type, text) = broadcast::read_server_message(&mut r)?;
            ServerPacket::ServerMessage { message_type, text }
        }
        opcode::SMSG_ZONE_UNDER_ATTACK => ServerPacket::ZoneUnderAttack {
            area_id: broadcast::read_zone_under_attack(&mut r)?,
        },
        opcode::SMSG_DEFENSE_MESSAGE => {
            let (zone_id, text) = broadcast::read_defense_message(&mut r)?;
            ServerPacket::DefenseMessage { zone_id, text }
        }
        opcode::SMSG_CHAT_RESTRICTED => ServerPacket::ChatRestricted,
        opcode::SMSG_PLAYED_TIME => {
            let (total, level) = chat::read_played_time(&mut r)?;
            ServerPacket::PlayedTime { total, level }
        }
        opcode::MSG_RANDOM_ROLL => {
            let (min, max, roll, guid) = chat::read_random_roll(&mut r)?;
            ServerPacket::RandomRoll {
                min,
                max,
                roll,
                guid,
            }
        }
        opcode::SMSG_INVENTORY_CHANGE_FAILURE => {
            let (reason, required_level, item_guid, bag_slot) =
                items::read_inventory_change_failure(&mut r)?;
            ServerPacket::InventoryChangeFailure {
                reason,
                required_level,
                item_guid,
                bag_slot,
            }
        }
        opcode::SMSG_INITIAL_SPELLS => {
            let (spell_ids, cooldowns) = spellbook::read_initial_spells(&mut r)?;
            ServerPacket::InitialSpells {
                spell_ids,
                cooldowns,
            }
        }
        opcode::SMSG_ACTION_BUTTONS => ServerPacket::ActionButtons {
            buttons: action_bar::read_action_buttons(&mut r)?,
        },
        opcode::SMSG_LEARNED_SPELL => ServerPacket::LearnedSpell {
            spell_id: spellbook::read_learned_spell(&mut r)?,
        },
        opcode::SMSG_REMOVED_SPELL => ServerPacket::RemovedSpell {
            spell_id: spellbook::read_removed_spell(&mut r)?,
        },
        opcode::SMSG_SUPERCEDED_SPELL => {
            let (old_spell_id, new_spell_id) = spellbook::read_superceded_spell(&mut r)?;
            ServerPacket::SupercededSpell {
                old_spell_id,
                new_spell_id,
            }
        }
        opcode::SMSG_CAST_RESULT => {
            let (spell_id, outcome) = spells::read_cast_result(&mut r)?;
            ServerPacket::CastResult { spell_id, outcome }
        }
        opcode::SMSG_PET_SPELLS => ServerPacket::PetSpells(pet::read_pet_spells(&mut r)?),
        opcode::SMSG_PET_MODE => ServerPacket::PetMode(pet::read_pet_mode(&mut r)?),
        opcode::SMSG_PET_ACTION_FEEDBACK => ServerPacket::PetActionFeedback {
            reason: pet::read_pet_action_feedback(&mut r)?,
        },
        opcode::SMSG_PET_CAST_FAILED => {
            let (spell_id, outcome) = pet::read_pet_cast_failed(&mut r)?;
            ServerPacket::PetCastFailed { spell_id, outcome }
        }
        opcode::SMSG_PET_TAME_FAILURE => ServerPacket::PetTameFailure {
            reason: pet::read_pet_tame_failure(&mut r)?,
        },
        // Empty bodies on both sides: vmangos writes nothing and the reference reads nothing.
        opcode::SMSG_PET_NAME_INVALID => ServerPacket::PetNameInvalid,
        opcode::SMSG_PET_BROKEN => ServerPacket::PetBroken,
        opcode::SMSG_PET_ACTION_SOUND => {
            let (pet_guid, talk) = pet::read_pet_action_sound(&mut r)?;
            ServerPacket::PetActionSound { pet_guid, talk }
        }
        opcode::SMSG_PET_DISMISS_SOUND => {
            let (model_id, position) = pet::read_pet_dismiss_sound(&mut r)?;
            ServerPacket::PetDismissSound { model_id, position }
        }
        opcode::SMSG_ATTACKSTART => {
            let (attacker, victim) = attack::read_attack_start(&mut r)?;
            ServerPacket::AttackStart { attacker, victim }
        }
        opcode::SMSG_ATTACKSTOP => {
            let (attacker, victim) = attack::read_attack_stop(&mut r)?;
            ServerPacket::AttackStop { attacker, victim }
        }
        opcode::SMSG_ATTACKERSTATEUPDATE => {
            ServerPacket::AttackerState(attack::read_attacker_state(&mut r)?)
        }
        // The empty swing refusals. `SMSG_ATTACKSWING_NOTSTANDING` (`0x147`) is absent on purpose:
        // the reference never registers it and vmangos never sends it.
        opcode::SMSG_ATTACKSWING_NOTINRANGE => {
            ServerPacket::AttackSwingError(AttackSwingError::NotInRange)
        }
        opcode::SMSG_ATTACKSWING_BADFACING => {
            ServerPacket::AttackSwingError(AttackSwingError::BadFacing)
        }
        opcode::SMSG_ATTACKSWING_DEADTARGET | opcode::SMSG_ATTACKSWING_CANT_ATTACK => {
            ServerPacket::AttackSwingError(AttackSwingError::DeadOrUnattackable)
        }
        // The family's fourth arm, registered with the spell handlers: same empty body, same act.
        opcode::SMSG_CANCEL_COMBAT => ServerPacket::CancelCombat,
        opcode::SMSG_FEIGN_DEATH_RESISTED => ServerPacket::FeignDeathResisted,
        opcode::SMSG_AI_REACTION => {
            let (unit, reaction) = attack::read_ai_reaction(&mut r)?;
            ServerPacket::AiReaction { unit, reaction }
        }
        opcode::SMSG_SPELL_START => ServerPacket::SpellStart(spells::read_spell_start(&mut r)?),
        opcode::SMSG_SPELL_GO => ServerPacket::SpellGo(spells::read_spell_go(&mut r)?),
        opcode::SMSG_SPELL_UPDATE_CHAIN_TARGETS => {
            ServerPacket::SpellChainTargets(spells::read_spell_chain_targets(&mut r)?)
        }
        opcode::SMSG_SPELL_FAILED_OTHER => {
            let (caster, spell_id) = spells::read_spell_failed_other(&mut r)?;
            ServerPacket::SpellFailedOther { caster, spell_id }
        }
        opcode::SMSG_SPELL_DELAYED => {
            let (caster, delay_ms) = spells::read_spell_delayed(&mut r)?;
            ServerPacket::SpellDelayed { caster, delay_ms }
        }
        opcode::SMSG_CANCEL_AUTO_REPEAT => ServerPacket::CancelAutoRepeat,
        opcode::SMSG_SPELL_COOLDOWN => {
            let (caster, cooldowns) = spellbook::read_spell_cooldown(&mut r)?;
            ServerPacket::SpellCooldownList { caster, cooldowns }
        }
        opcode::SMSG_ITEM_COOLDOWN => {
            let (item_guid, spell_id) = spellbook::read_item_cooldown(&mut r)?;
            ServerPacket::ItemCooldown {
                item_guid,
                spell_id,
            }
        }
        opcode::SMSG_ITEM_TIME_UPDATE => {
            let (item_guid, seconds) = items::read_item_time(&mut r)?;
            ServerPacket::ItemTime { item_guid, seconds }
        }
        opcode::SMSG_ITEM_ENCHANT_TIME_UPDATE => {
            let (item_guid, slot, seconds) = items::read_item_enchant_time(&mut r)?;
            ServerPacket::ItemEnchantTime {
                item_guid,
                slot,
                seconds,
            }
        }
        // One body, two tables: the opcode picks, as `0x6e9950`'s `cmp edi,0x267` does.
        opcode::SMSG_SET_FLAT_SPELL_MODIFIER | opcode::SMSG_SET_PCT_SPELL_MODIFIER => {
            let (mask_bit, op, value) = spells::read_set_spell_modifier(&mut r)?;
            ServerPacket::SpellModifier {
                flat: opcode == opcode::SMSG_SET_FLAT_SPELL_MODIFIER,
                mask_bit,
                op,
                value,
            }
        }
        opcode::SMSG_COOLDOWN_EVENT => {
            let (spell_id, caster) = spellbook::read_cooldown_event(&mut r)?;
            ServerPacket::CooldownEvent { spell_id, caster }
        }
        opcode::SMSG_CLEAR_COOLDOWN => {
            let (spell_id, caster) = spellbook::read_cooldown_event(&mut r)?;
            ServerPacket::ClearCooldown { spell_id, caster }
        }
        opcode::SMSG_COOLDOWN_CHEAT => ServerPacket::CooldownCheat {
            caster: spellbook::read_cooldown_cheat(&mut r)?,
        },
        opcode::MSG_CHANNEL_START => {
            let (spell_id, duration_ms) = spells::read_channel_start(&mut r)?;
            ServerPacket::ChannelStart {
                spell_id,
                duration_ms,
            }
        }
        opcode::MSG_CHANNEL_UPDATE => ServerPacket::ChannelUpdate {
            remaining_ms: spells::read_channel_update(&mut r)?,
        },
        opcode::SMSG_UPDATE_AURA_DURATION => {
            let (slot, remaining_ms) = spells::read_update_aura_duration(&mut r)?;
            ServerPacket::UpdateAuraDuration { slot, remaining_ms }
        }
        opcode::SMSG_PLAY_SPELL_VISUAL => {
            let (unit, kit_id) = spells::read_play_spell_visual(&mut r)?;
            ServerPacket::PlaySpellVisual { unit, kit_id }
        }
        opcode::SMSG_SPELLNONMELEEDAMAGELOG => {
            ServerPacket::SpellDamageLog(combat_log::read_spell_damage_log(&mut r)?)
        }
        opcode::SMSG_PERIODICAURALOG => {
            ServerPacket::PeriodicAuraLog(combat_log::read_periodic_aura_log(&mut r)?)
        }
        opcode::SMSG_SPELLDAMAGESHIELD => {
            ServerPacket::DamageShield(combat_log::read_damage_shield(&mut r)?)
        }
        opcode::SMSG_SPELLHEALLOG => {
            ServerPacket::SpellHealLog(combat_log::read_spell_heal_log(&mut r)?)
        }
        opcode::SMSG_SPELLENERGIZELOG => {
            ServerPacket::SpellEnergizeLog(combat_log::read_spell_energize_log(&mut r)?)
        }
        opcode::SMSG_ENVIRONMENTALDAMAGELOG => {
            ServerPacket::EnvironmentalDamageLog(combat_log::read_environmental_damage_log(&mut r)?)
        }
        opcode::SMSG_SPELLLOGMISS => {
            ServerPacket::SpellLogMiss(combat_log::read_spell_log_miss(&mut r)?)
        }
        opcode::SMSG_PARTYKILLLOG => {
            ServerPacket::PartyKillLog(combat_log::read_party_kill_log(&mut r)?)
        }
        opcode::SMSG_SPELLINSTAKILLLOG => {
            ServerPacket::SpellInstaKillLog(combat_log::read_spell_insta_kill_log(&mut r)?)
        }
        // One body, two sentences: only the opcode tells them apart.
        opcode::SMSG_PROCRESIST => {
            ServerPacket::ProcResist(combat_log::read_spell_outcome_log(&mut r)?)
        }
        opcode::SMSG_SPELLORDAMAGE_IMMUNE => {
            ServerPacket::SpellOrDamageImmune(combat_log::read_spell_outcome_log(&mut r)?)
        }
        opcode::SMSG_SPELLDISPELLOG => {
            ServerPacket::SpellDispelLog(combat_log::read_spell_dispel_log(&mut r)?)
        }
        opcode::SMSG_DISPEL_FAILED => {
            ServerPacket::DispelFailed(combat_log::read_dispel_failed(&mut r)?)
        }
        opcode::SMSG_ENCHANTMENTLOG => {
            ServerPacket::EnchantmentLog(combat_log::read_enchantment_log(&mut r)?)
        }
        opcode::SMSG_SPELLLOGEXECUTE => {
            ServerPacket::SpellLogExecute(combat_log::read_spell_log_execute(&mut r)?)
        }
        opcode::SMSG_LOG_XPGAIN => ServerPacket::XpGain(progression::read_xp_gain(&mut r)?),
        opcode::SMSG_EXPLORATION_EXPERIENCE => {
            ServerPacket::ExplorationXp(progression::read_exploration_xp(&mut r)?)
        }
        opcode::SMSG_LEVELUP_INFO => {
            ServerPacket::LevelUp(progression::read_level_up_info(&mut r)?)
        }
        opcode::SMSG_QUESTGIVER_STATUS => {
            let (npc, status) = quest::read_questgiver_status(&mut r)?;
            ServerPacket::QuestGiverStatus { npc, status }
        }
        opcode::SMSG_QUESTGIVER_QUEST_LIST => {
            ServerPacket::QuestGiverQuestList(quest::read_questgiver_quest_list(&mut r)?)
        }
        opcode::SMSG_QUESTGIVER_QUEST_DETAILS => {
            ServerPacket::QuestGiverDetails(quest::read_questgiver_quest_details(&mut r)?)
        }
        opcode::SMSG_QUESTGIVER_REQUEST_ITEMS => {
            ServerPacket::QuestGiverRequestItems(quest::read_questgiver_request_items(&mut r)?)
        }
        opcode::SMSG_QUESTGIVER_OFFER_REWARD => {
            ServerPacket::QuestGiverOfferReward(quest::read_questgiver_offer_reward(&mut r)?)
        }
        opcode::SMSG_QUESTGIVER_QUEST_COMPLETE => {
            ServerPacket::QuestGiverComplete(quest::read_questgiver_quest_complete(&mut r)?)
        }
        opcode::SMSG_QUESTGIVER_QUEST_INVALID => ServerPacket::QuestGiverInvalid {
            msg: quest::read_questgiver_quest_invalid(&mut r)?,
        },
        opcode::SMSG_QUESTGIVER_QUEST_FAILED => {
            let (quest_id, reason) = quest::read_questgiver_quest_failed(&mut r)?;
            ServerPacket::QuestGiverFailed { quest_id, reason }
        }
        opcode::SMSG_QUEST_QUERY_RESPONSE => {
            ServerPacket::QuestQueryResponse(Box::new(quest::read_quest_query_response(&mut r)?))
        }
        opcode::SMSG_QUESTLOG_FULL => ServerPacket::QuestLogFull,
        opcode::MSG_QUEST_PUSH_RESULT => {
            ServerPacket::QuestPushResult(quest::read_quest_push_result(&mut r)?)
        }
        opcode::SMSG_QUEST_CONFIRM_ACCEPT => {
            ServerPacket::QuestConfirmAccept(quest::read_quest_confirm_accept(&mut r)?)
        }
        opcode::SMSG_QUESTUPDATE_COMPLETE => ServerPacket::QuestUpdateComplete {
            quest_id: quest::read_quest_update_complete(&mut r)?,
        },
        opcode::SMSG_QUESTUPDATE_FAILED => ServerPacket::QuestUpdateFailed {
            quest_id: quest::read_quest_update_failed(&mut r)?,
        },
        opcode::SMSG_QUESTUPDATE_FAILEDTIMER => ServerPacket::QuestUpdateFailedTimer {
            quest_id: quest::read_quest_update_failedtimer(&mut r)?,
        },
        opcode::SMSG_QUESTUPDATE_ADD_KILL => {
            let (quest_id, entry, count, required, guid) =
                quest::read_quest_update_add_kill(&mut r)?;
            ServerPacket::QuestUpdateAddKill {
                quest_id,
                entry,
                count,
                required,
                guid,
            }
        }
        opcode::SMSG_QUESTUPDATE_ADD_ITEM => {
            let (item_id, count) = quest::read_quest_update_add_item(&mut r)?;
            ServerPacket::QuestUpdateAddItem { item_id, count }
        }
        opcode::SMSG_GOSSIP_MESSAGE => {
            let (npc, text_id, options, quests) = gossip::read_gossip_message(&mut r)?;
            ServerPacket::GossipMessage {
                npc,
                text_id,
                options,
                quests,
            }
        }
        opcode::SMSG_GOSSIP_COMPLETE => ServerPacket::GossipComplete,
        opcode::SMSG_GOSSIP_POI => ServerPacket::GossipPoi(gossip::read_gossip_poi(&mut r)?),
        opcode::SMSG_NPC_TEXT_UPDATE => {
            let (text_id, blocks) = gossip::read_npc_text_update(&mut r)?;
            ServerPacket::NpcText { text_id, blocks }
        }
        opcode::SMSG_LIST_INVENTORY => {
            let (vendor, items) = vendor::read_list_inventory(&mut r)?;
            ServerPacket::VendorList { vendor, items }
        }
        opcode::SMSG_BUY_ITEM => {
            let (vendor, slot, new_count, purchase_count) = vendor::read_buy_item(&mut r)?;
            ServerPacket::BuyItem {
                vendor,
                slot,
                new_count,
                purchase_count,
            }
        }
        opcode::SMSG_SELL_ITEM => {
            let (vendor, item_guid, reason) = vendor::read_sell_item(&mut r)?;
            ServerPacket::SellItemResult {
                vendor,
                item_guid,
                reason,
            }
        }
        opcode::SMSG_BUY_FAILED => {
            let (vendor, item_entry, reason) = vendor::read_buy_failed(&mut r)?;
            ServerPacket::BuyFailed {
                vendor,
                item_entry,
                reason,
            }
        }
        opcode::SMSG_SHOW_BANK => {
            let banker = bank::read_show_bank(&mut r)?;
            ServerPacket::ShowBank { banker }
        }
        opcode::SMSG_BUY_BANK_SLOT_RESULT => {
            let result = bank::read_buy_bank_slot_result(&mut r)?;
            ServerPacket::BuyBankSlotResult { result }
        }
        opcode::SMSG_TRAINER_LIST => {
            let (trainer, trainer_type, services, title) = trainer::read_trainer_list(&mut r)?;
            ServerPacket::TrainerList {
                trainer,
                trainer_type,
                services,
                title,
            }
        }
        opcode::SMSG_TRAINER_BUY_SUCCEEDED => {
            let (trainer, spell_id) = trainer::read_trainer_buy_succeeded(&mut r)?;
            ServerPacket::TrainerBuySucceeded { trainer, spell_id }
        }
        opcode::SMSG_TRAINER_BUY_FAILED => {
            let (trainer, spell_id, error) = trainer::read_trainer_buy_failed(&mut r)?;
            ServerPacket::TrainerBuyFailed {
                trainer,
                spell_id,
                error,
            }
        }
        opcode::MSG_LIST_STABLED_PETS => {
            let (npc, num_stable_slots, pets) = stable::read_list_stabled_pets(&mut r)?;
            ServerPacket::ListStabledPets {
                npc,
                num_stable_slots,
                pets,
            }
        }
        opcode::SMSG_INVALIDATE_PLAYER => ServerPacket::InvalidatePlayer {
            guid: read_u64_le(&mut r)?,
        },
        opcode::SMSG_STABLE_RESULT => {
            let result = stable::read_stable_result(&mut r)?;
            ServerPacket::StableResult { result }
        }
        opcode::SMSG_LOOT_RESPONSE => {
            let (guid, body) = loot::read_loot_response(&mut r)?;
            match body {
                loot::LootResponseBody::Items {
                    loot_type,
                    gold,
                    items,
                } => ServerPacket::LootResponse {
                    guid,
                    loot_type,
                    gold,
                    items,
                },
                loot::LootResponseBody::Error { error } => ServerPacket::LootError { guid, error },
            }
        }
        opcode::SMSG_LOOT_RELEASE_RESPONSE => {
            let (guid, result) = loot::read_loot_release_response(&mut r)?;
            ServerPacket::LootReleaseResponse { guid, result }
        }
        opcode::SMSG_LOOT_REMOVED => ServerPacket::LootRemoved {
            slot: loot::read_loot_removed(&mut r)?,
        },
        opcode::SMSG_LOOT_MONEY_NOTIFY => ServerPacket::LootMoneyNotify {
            amount: loot::read_loot_money_notify(&mut r)?,
        },
        opcode::SMSG_LOOT_CLEAR_MONEY => ServerPacket::LootClearMoney,
        opcode::SMSG_LOOT_START_ROLL => {
            ServerPacket::LootStartRoll(loot::read_loot_start_roll(&mut r)?)
        }
        opcode::SMSG_LOOT_ROLL => ServerPacket::LootRoll(loot::read_loot_roll(&mut r)?),
        opcode::SMSG_LOOT_ROLL_WON => ServerPacket::LootRollWon(loot::read_loot_roll_won(&mut r)?),
        opcode::SMSG_LOOT_ALL_PASSED => {
            ServerPacket::LootAllPassed(loot::read_loot_all_passed(&mut r)?)
        }
        opcode::SMSG_LOOT_MASTER_LIST => ServerPacket::LootMasterList {
            candidates: loot::read_loot_master_list(&mut r)?,
        },
        opcode::SMSG_ITEM_PUSH_RESULT => {
            ServerPacket::ItemPushResult(loot::read_item_push_result(&mut r)?)
        }
        opcode::MSG_CORPSE_QUERY => {
            ServerPacket::CorpseQuery(death::read_corpse_query_response(&mut r)?)
        }
        opcode::SMSG_DURABILITY_DAMAGE_DEATH => ServerPacket::DurabilityDamageDeath,
        opcode::SMSG_CORPSE_RECLAIM_DELAY => ServerPacket::CorpseReclaimDelay {
            delay_ms: death::read_corpse_reclaim_delay(&mut r)?,
        },
        opcode::SMSG_RESURRECT_REQUEST => {
            ServerPacket::ResurrectRequest(death::read_resurrect_request(&mut r)?)
        }
        opcode::SMSG_SPIRIT_HEALER_CONFIRM => ServerPacket::SpiritHealerConfirm {
            npc: death::read_spirit_healer_confirm(&mut r)?,
        },
        // The acked mode family, all eight `packed guid, u32 counter`
        // (`MovementPacketSender.cpp:342-366`); unacked, the server never applies the change.
        opcode::SMSG_FORCE_MOVE_ROOT
        | opcode::SMSG_FORCE_MOVE_UNROOT
        | opcode::SMSG_MOVE_WATER_WALK
        | opcode::SMSG_MOVE_LAND_WALK
        | opcode::SMSG_MOVE_FEATHER_FALL
        | opcode::SMSG_MOVE_NORMAL_FALL
        | opcode::SMSG_MOVE_SET_HOVER
        | opcode::SMSG_MOVE_UNSET_HOVER => {
            let (mode, apply) = match opcode {
                opcode::SMSG_FORCE_MOVE_ROOT => (MoveMode::Root, true),
                opcode::SMSG_FORCE_MOVE_UNROOT => (MoveMode::Root, false),
                opcode::SMSG_MOVE_WATER_WALK => (MoveMode::WaterWalk, true),
                opcode::SMSG_MOVE_LAND_WALK => (MoveMode::WaterWalk, false),
                opcode::SMSG_MOVE_FEATHER_FALL => (MoveMode::FeatherFall, true),
                opcode::SMSG_MOVE_NORMAL_FALL => (MoveMode::FeatherFall, false),
                opcode::SMSG_MOVE_SET_HOVER => (MoveMode::Hover, true),
                _ => (MoveMode::Hover, false),
            };
            let guid = read_packed_guid(&mut r)?;
            let counter = read_u32_le(&mut r)?;
            ServerPacket::MoveMode {
                guid,
                counter,
                mode,
                apply,
            }
        }
        // The observer mode family: twelve bare packed guids for any unit, nothing to ack
        // (vmangos `SendMovementFlagChangeToAll`; the reference's one handler `0x603c80`).
        opcode::SMSG_SPLINE_MOVE_ROOT
        | opcode::SMSG_SPLINE_MOVE_UNROOT
        | opcode::SMSG_SPLINE_MOVE_WATER_WALK
        | opcode::SMSG_SPLINE_MOVE_LAND_WALK
        | opcode::SMSG_SPLINE_MOVE_FEATHER_FALL
        | opcode::SMSG_SPLINE_MOVE_NORMAL_FALL
        | opcode::SMSG_SPLINE_MOVE_SET_HOVER
        | opcode::SMSG_SPLINE_MOVE_UNSET_HOVER
        | opcode::SMSG_SPLINE_MOVE_START_SWIM
        | opcode::SMSG_SPLINE_MOVE_STOP_SWIM
        | opcode::SMSG_SPLINE_MOVE_SET_RUN_MODE
        | opcode::SMSG_SPLINE_MOVE_SET_WALK_MODE => {
            let (mode, apply) = match opcode {
                opcode::SMSG_SPLINE_MOVE_ROOT => (SplineMode::Root, true),
                opcode::SMSG_SPLINE_MOVE_UNROOT => (SplineMode::Root, false),
                opcode::SMSG_SPLINE_MOVE_WATER_WALK => (SplineMode::WaterWalk, true),
                opcode::SMSG_SPLINE_MOVE_LAND_WALK => (SplineMode::WaterWalk, false),
                opcode::SMSG_SPLINE_MOVE_FEATHER_FALL => (SplineMode::FeatherFall, true),
                opcode::SMSG_SPLINE_MOVE_NORMAL_FALL => (SplineMode::FeatherFall, false),
                opcode::SMSG_SPLINE_MOVE_SET_HOVER => (SplineMode::Hover, true),
                opcode::SMSG_SPLINE_MOVE_UNSET_HOVER => (SplineMode::Hover, false),
                opcode::SMSG_SPLINE_MOVE_START_SWIM => (SplineMode::Swimming, true),
                opcode::SMSG_SPLINE_MOVE_STOP_SWIM => (SplineMode::Swimming, false),
                // The one inverted pair: RUN_MODE clears `MOVEFLAG_WALK_MODE`, because the
                // reference's `0x617e80` feeds the opcode's bool to `SetRunMode 0x7c71c0`.
                opcode::SMSG_SPLINE_MOVE_SET_WALK_MODE => (SplineMode::WalkMode, true),
                _ => (SplineMode::WalkMode, false),
            };
            ServerPacket::SplineMoveMode {
                guid: read_packed_guid(&mut r)?,
                mode,
                apply,
            }
        }
        // `packed guid, u32 counter`, then `vcos, vsin, speedXY, speedZ`
        // (`MovementPacketSender.cpp:261-277`): the jump tail the ack echoes, but direction-first
        // where `JumpInfo` serializes `zspeed` first, so the reads are spelled out.
        opcode::SMSG_MOVE_KNOCK_BACK => {
            let guid = read_packed_guid(&mut r)?;
            let counter = read_u32_le(&mut r)?;
            let cos_angle = read_f32_le(&mut r)?;
            let sin_angle = read_f32_le(&mut r)?;
            let xy_speed = read_f32_le(&mut r)?;
            let zspeed = read_f32_le(&mut r)?;
            ServerPacket::KnockBack {
                guid,
                counter,
                launch: JumpInfo {
                    zspeed,
                    cos_angle,
                    sin_angle,
                    xy_speed,
                },
            }
        }
        opcode::SMSG_INITIALIZE_FACTIONS => {
            let count = read_u32_le(&mut r)?;
            // The array is exactly `MAX_FACTION_COUNT` 64 entries long (`FACTION_LIST_LEN`).
            let mut standings =
                Vec::with_capacity(capacity_hint(count, super::reputation::FACTION_LIST_LEN));
            for _ in 0..count {
                let flags = read_u8(&mut r)?;
                let standing = read_u32_le(&mut r)? as i32;
                standings.push((flags, standing));
            }
            ServerPacket::InitializeFactions { standings }
        }
        opcode::SMSG_SET_FACTION_STANDING => {
            let count = read_u32_le(&mut r)?;
            // At most one row per reputation-list slot (`FACTION_LIST_LEN`).
            let mut standings =
                Vec::with_capacity(capacity_hint(count, super::reputation::FACTION_LIST_LEN));
            for _ in 0..count {
                let list_id = read_u32_le(&mut r)?;
                let standing = read_u32_le(&mut r)? as i32;
                standings.push((list_id, standing));
            }
            ServerPacket::SetFactionStanding { standings }
        }
        opcode::SMSG_SET_FACTION_VISIBLE => ServerPacket::SetFactionVisible {
            list_id: read_u32_le(&mut r)?,
        },
        opcode::SMSG_NAME_QUERY_RESPONSE => {
            let guid = read_u64_le(&mut r)?;
            let name = read_cstring(&mut r)?;
            let _realm = read_cstring(&mut r)?; // cross-realm BG name; empty on a single realm
            ServerPacket::NameQueryResponse {
                guid,
                name,
                race: read_u32_le(&mut r)?,
                gender: read_u32_le(&mut r)?,
                class: read_u32_le(&mut r)?,
            }
        }
        opcode::SMSG_CREATURE_QUERY_RESPONSE => {
            let entry = read_u32_le(&mut r)?;
            // A miss is the lone entry echoed with its top bit set; nothing follows.
            if entry & 0x8000_0000 != 0 {
                ServerPacket::CreatureQueryResponse {
                    entry: entry & 0x7FFF_FFFF,
                    info: None,
                }
            } else {
                let name = read_cstring(&mut r)?;
                for _ in 0..3 {
                    let _ = read_cstring(&mut r)?; // name2..name4, always empty in 5875
                }
                let subname = read_cstring(&mut r)?;
                // `HandleCreatureQueryOpcode`: seven `u32`s (type_flags, type, pet_family, rank,
                // unk, pet_spell_list_id, display_id), then civilian and racial_leader as `u8`s.
                let type_flags = read_u32_le(&mut r)?;
                let creature_type = read_u32_le(&mut r)?;
                let pet_family = read_u32_le(&mut r)?;
                let rank = read_u32_le(&mut r)?;
                let _unk = read_u32_le(&mut r)?;
                let _pet_spell_list = read_u32_le(&mut r)?;
                let display_id = read_u32_le(&mut r)?;
                let civilian = read_u8(&mut r)? != 0;
                let racial_leader = read_u8(&mut r)? != 0;
                ServerPacket::CreatureQueryResponse {
                    entry,
                    info: Some(CreatureQueryInfo {
                        name,
                        subname,
                        creature_type,
                        pet_family,
                        rank,
                        type_flags,
                        display_id,
                        civilian,
                        racial_leader,
                    }),
                }
            }
        }
        // `u32 petNumber, cstring name, u32 nameTimestamp` (`Pet.cpp:79-84`); the timestamp only
        // ages the reference's on-disk pet-name cache.
        opcode::SMSG_PET_NAME_QUERY_RESPONSE => {
            let pet_number = read_u32_le(&mut r)?;
            let name = read_cstring(&mut r)?;
            let _name_timestamp = read_u32_le(&mut r)?;
            ServerPacket::PetNameQueryResponse { pet_number, name }
        }
        opcode::SMSG_GAMEOBJECT_QUERY_RESPONSE => {
            let (entry, info) = gameobject::read_gameobject_query_response(&mut r)?;
            ServerPacket::GameObjectQueryResponse { entry, info }
        }
        opcode::SMSG_PAGE_TEXT_QUERY_RESPONSE => {
            let (page_id, text, next_page_id) = page_text::read_page_text_query_response(&mut r)?;
            ServerPacket::PageTextQueryResponse {
                page_id,
                text,
                next_page_id,
            }
        }
        opcode::SMSG_GAMEOBJECT_CUSTOM_ANIM => {
            let (guid, anim_id) = gameobject::read_gameobject_custom_anim(&mut r)?;
            ServerPacket::GameObjectCustomAnim { guid, anim_id }
        }
        opcode::SMSG_GAMEOBJECT_DESPAWN_ANIM => {
            let guid = gameobject::read_gameobject_despawn_anim(&mut r)?;
            ServerPacket::GameObjectDespawnAnim { guid }
        }
        opcode::SMSG_OPEN_CONTAINER => ServerPacket::OpenContainer {
            item: items::read_open_container(&mut r)?,
        },
        opcode::SMSG_INSPECT => ServerPacket::Inspect {
            guid: items::read_inspect(&mut r)?,
        },
        opcode::SMSG_STANDSTATE_UPDATE => ServerPacket::StandStateUpdate {
            state: pose::read_stand_state_update(&mut r)?,
        },
        opcode::SMSG_FISH_NOT_HOOKED => ServerPacket::FishNotHooked,
        opcode::SMSG_FISH_ESCAPED => ServerPacket::FishEscaped,
        opcode::SMSG_LOGOUT_COMPLETE => ServerPacket::LogoutComplete,
        opcode::SMSG_LOGOUT_RESPONSE => ServerPacket::LogoutResponse {
            reason: read_u32_le(&mut r)?,
            instant: read_u8(&mut r)? != 0,
        },
        opcode::SMSG_LOGOUT_CANCEL_ACK => ServerPacket::LogoutCancelAck,
        opcode::SMSG_PONG => ServerPacket::Pong {
            sequence: read_u32_le(&mut r)?,
        },
        opcode::SMSG_FORCE_WALK_SPEED_CHANGE => read_force_speed(SpeedKind::Walk, &mut r)?,
        opcode::SMSG_FORCE_RUN_SPEED_CHANGE => read_force_speed(SpeedKind::Run, &mut r)?,
        opcode::SMSG_FORCE_RUN_BACK_SPEED_CHANGE => read_force_speed(SpeedKind::RunBack, &mut r)?,
        opcode::SMSG_FORCE_SWIM_SPEED_CHANGE => read_force_speed(SpeedKind::Swim, &mut r)?,
        opcode::SMSG_FORCE_SWIM_BACK_SPEED_CHANGE => read_force_speed(SpeedKind::SwimBack, &mut r)?,
        opcode::SMSG_FORCE_TURN_RATE_CHANGE => read_force_speed(SpeedKind::TurnRate, &mut r)?,
        opcode::SMSG_GROUP_INVITE => ServerPacket::GroupInvite {
            inviter: group::read_group_invite(&mut r)?,
        },
        opcode::SMSG_GROUP_DECLINE => ServerPacket::GroupDecline {
            name: group::read_group_decline(&mut r)?,
        },
        opcode::SMSG_GROUP_UNINVITE => ServerPacket::GroupUninvited,
        opcode::SMSG_GROUP_SET_LEADER => ServerPacket::GroupLeaderChanged {
            name: group::read_group_set_leader(&mut r)?,
        },
        opcode::SMSG_GROUP_DESTROYED => ServerPacket::GroupDestroyed,
        opcode::SMSG_GROUP_LIST => {
            let (group_type, own_flags, members, leader, loot) = group::read_group_list(&mut r)?;
            ServerPacket::GroupList {
                group_type,
                own_flags,
                members,
                leader,
                loot,
            }
        }
        opcode::SMSG_PARTY_COMMAND_RESULT => {
            let (operation, member, result) = group::read_party_command_result(&mut r)?;
            ServerPacket::PartyCommandResult {
                operation,
                member,
                result,
            }
        }
        opcode::SMSG_PARTY_MEMBER_STATS | opcode::SMSG_PARTY_MEMBER_STATS_FULL => {
            let (guid, info) = group::read_party_member_stats(&mut r)?;
            ServerPacket::PartyMemberStats {
                guid,
                full: opcode == opcode::SMSG_PARTY_MEMBER_STATS_FULL,
                info: Box::new(info),
            }
        }
        opcode::MSG_MINIMAP_PING => {
            let (guid, x, y) = group::read_minimap_ping(&mut r)?;
            ServerPacket::MinimapPing { guid, x, y }
        }
        opcode::MSG_RAID_TARGET_UPDATE => match group::read_raid_target_update(&mut r)? {
            group::RaidTargetUpdate::Delta { icon, guid } => {
                ServerPacket::RaidTargetSet { icon, guid }
            }
            group::RaidTargetUpdate::List(entries) => ServerPacket::RaidTargetList { entries },
        },
        opcode::SMSG_RAID_INSTANCE_INFO => ServerPacket::RaidInstanceInfo {
            entries: group::read_raid_instance_info(&mut r)?,
        },
        opcode::MSG_RAID_READY_CHECK => match group::read_ready_check(&mut r)? {
            group::ReadyCheck::Started => ServerPacket::ReadyCheckRequest,
            group::ReadyCheck::Answer { guid, ready } => {
                ServerPacket::ReadyCheckAnswer { guid, ready }
            }
        },
        // The lockout family, read in the client's own field order.
        opcode::SMSG_RAID_INSTANCE_MESSAGE => ServerPacket::RaidInstanceMessage {
            message: instance::read_raid_instance_message(&mut r)?,
        },
        opcode::SMSG_INSTANCE_SAVE_CREATED => ServerPacket::InstanceSaveCreated {
            flag: instance::read_u32_body(&mut r)?,
        },
        opcode::SMSG_INSTANCE_RESET => ServerPacket::InstanceReset {
            map: instance::read_u32_body(&mut r)?,
        },
        opcode::SMSG_INSTANCE_RESET_FAILED => ServerPacket::InstanceResetFailed {
            failure: instance::read_instance_reset_failed(&mut r)?,
        },
        opcode::SMSG_UPDATE_LAST_INSTANCE => ServerPacket::UpdateLastInstance {
            map: instance::read_u32_body(&mut r)?,
        },
        opcode::SMSG_UPDATE_INSTANCE_OWNERSHIP => ServerPacket::UpdateInstanceOwnership {
            owns: instance::read_u32_body(&mut r)?,
        },
        opcode::SMSG_DUEL_REQUESTED => {
            let req = duel::read_duel_requested(&mut r)?;
            ServerPacket::DuelRequested {
                arbiter: req.arbiter,
                challenger: req.challenger,
            }
        }
        opcode::SMSG_DUEL_OUTOFBOUNDS => ServerPacket::DuelOutOfBounds,
        opcode::SMSG_DUEL_INBOUNDS => ServerPacket::DuelInBounds,
        opcode::SMSG_DUEL_COMPLETE => ServerPacket::DuelComplete {
            started: duel::read_duel_complete(&mut r)?,
        },
        opcode::SMSG_DUEL_WINNER => {
            let w = duel::read_duel_winner(&mut r)?;
            ServerPacket::DuelWinner {
                fled: w.fled,
                winner: w.winner,
                loser: w.loser,
            }
        }
        opcode::SMSG_DUEL_COUNTDOWN => ServerPacket::DuelCountdown {
            seconds: duel::read_duel_countdown(&mut r)?,
        },
        // `0x2D6` carries our 8-byte request and the 50-byte reply; only the reply reaches here.
        opcode::MSG_INSPECT_HONOR_STATS => {
            ServerPacket::InspectHonorStats(pvp::read_inspect_honor_stats(&mut r)?)
        }
        opcode::SMSG_PVP_CREDIT => ServerPacket::PvpCredit(pvp::read_pvp_credit(&mut r)?),
        opcode::SMSG_START_MIRROR_TIMER => {
            ServerPacket::MirrorTimerStart(mirror_timer::read_start_mirror_timer(&mut r)?)
        }
        opcode::SMSG_PAUSE_MIRROR_TIMER => {
            let (kind, paused) = mirror_timer::read_pause_mirror_timer(&mut r)?;
            ServerPacket::MirrorTimerPause { kind, paused }
        }
        opcode::SMSG_STOP_MIRROR_TIMER => ServerPacket::MirrorTimerStop {
            kind: mirror_timer::read_stop_mirror_timer(&mut r)?,
        },
        opcode::SMSG_FRIEND_LIST => ServerPacket::FriendList {
            friends: social::read_friend_list(&mut r)?,
        },
        opcode::SMSG_IGNORE_LIST => ServerPacket::IgnoreList {
            guids: social::read_ignore_list(&mut r)?,
        },
        opcode::SMSG_FRIEND_STATUS => {
            ServerPacket::FriendStatus(social::read_friend_status(&mut r)?)
        }
        opcode::SMSG_WHO => ServerPacket::WhoResults(social::read_who(&mut r)?),
        opcode::SMSG_GUILD_QUERY_RESPONSE => {
            ServerPacket::GuildQueryResponse(guild::read_guild_query_response(&mut r)?)
        }
        opcode::SMSG_GUILD_ROSTER => ServerPacket::GuildRoster(guild::read_guild_roster(&mut r)?),
        opcode::SMSG_GUILD_EVENT => ServerPacket::GuildEvent(guild::read_guild_event(&mut r)?),
        opcode::SMSG_GUILD_COMMAND_RESULT => {
            ServerPacket::GuildCommandResult(guild::read_guild_command_result(&mut r)?)
        }
        opcode::SMSG_GUILD_INVITE => {
            let (inviter, guild) = guild::read_guild_invite(&mut r)?;
            ServerPacket::GuildInvite { inviter, guild }
        }
        opcode::SMSG_GUILD_DECLINE => ServerPacket::GuildDecline {
            name: guild::read_guild_decline(&mut r)?,
        },
        opcode::SMSG_GUILD_INFO => ServerPacket::GuildInfo(guild::read_guild_info(&mut r)?),
        // Guild founding; the two `MSG_` opcodes read a different body from the one they write.
        opcode::SMSG_PETITION_SHOWLIST => {
            ServerPacket::PetitionShowList(petition::read_petition_show_list(&mut r)?)
        }
        opcode::SMSG_PETITION_SHOW_SIGNATURES => {
            ServerPacket::PetitionShowSignatures(petition::read_petition_show_signatures(&mut r)?)
        }
        opcode::SMSG_PETITION_SIGN_RESULTS => {
            ServerPacket::PetitionSignResults(petition::read_petition_sign_results(&mut r)?)
        }
        opcode::SMSG_PETITION_QUERY_RESPONSE => {
            ServerPacket::PetitionQueryResponse(petition::read_petition_query_response(&mut r)?)
        }
        opcode::SMSG_TURN_IN_PETITION_RESULTS => ServerPacket::TurnInPetitionResults {
            result: petition::read_turn_in_petition_results(&mut r)?,
        },
        opcode::MSG_PETITION_DECLINE => ServerPacket::PetitionDeclined {
            player: petition::read_petition_decline(&mut r)?,
        },
        opcode::MSG_PETITION_RENAME => {
            ServerPacket::PetitionRenamed(petition::read_petition_rename(&mut r)?)
        }
        opcode::SMSG_SPLINE_SET_WALK_SPEED => read_spline_speed(SpeedKind::Walk, &mut r)?,
        opcode::SMSG_SPLINE_SET_RUN_SPEED => read_spline_speed(SpeedKind::Run, &mut r)?,
        opcode::SMSG_SPLINE_SET_RUN_BACK_SPEED => read_spline_speed(SpeedKind::RunBack, &mut r)?,
        opcode::SMSG_SPLINE_SET_SWIM_SPEED => read_spline_speed(SpeedKind::Swim, &mut r)?,
        opcode::SMSG_SPLINE_SET_SWIM_BACK_SPEED => read_spline_speed(SpeedKind::SwimBack, &mut r)?,
        opcode::SMSG_SPLINE_SET_TURN_RATE => read_spline_speed(SpeedKind::TurnRate, &mut r)?,
        opcode::MSG_MOVE_SET_WALK_SPEED => read_move_set_speed(SpeedKind::Walk, &mut r)?,
        opcode::MSG_MOVE_SET_RUN_SPEED => read_move_set_speed(SpeedKind::Run, &mut r)?,
        opcode::MSG_MOVE_SET_RUN_BACK_SPEED => read_move_set_speed(SpeedKind::RunBack, &mut r)?,
        opcode::MSG_MOVE_SET_SWIM_SPEED => read_move_set_speed(SpeedKind::Swim, &mut r)?,
        opcode::MSG_MOVE_SET_SWIM_BACK_SPEED => read_move_set_speed(SpeedKind::SwimBack, &mut r)?,
        opcode::MSG_MOVE_SET_TURN_RATE => read_move_set_speed(SpeedKind::TurnRate, &mut r)?,
        opcode::SMSG_MOUNTRESULT => ServerPacket::MountResult {
            mount: true,
            code: read_u32_le(&mut r)?,
        },
        opcode::SMSG_DISMOUNTRESULT => ServerPacket::MountResult {
            mount: false,
            code: read_u32_le(&mut r)?,
        },
        // A full 8-byte guid, not packed (vmangos `HandleMountSpecialAnimOpcode`).
        opcode::SMSG_MOUNTSPECIAL_ANIM => ServerPacket::MountSpecialAnim {
            guid: read_u64_le(&mut r)?,
        },
        // A packed guid, unlike the flourish, then `u8 allowMove` (`Misc.cpp:677-682`).
        opcode::SMSG_CLIENT_CONTROL_UPDATE => ServerPacket::ClientControlUpdate {
            mover: read_packed_guid(&mut r)?,
            allow_move: read_u8(&mut r)? != 0,
        },
        opcode::SMSG_SHOWTAXINODES => {
            let (window, flightmaster, nearest_node, known) = taxi::read_show_taxi_nodes(&mut r)?;
            ServerPacket::ShowTaxiNodes {
                window,
                flightmaster,
                nearest_node,
                known,
            }
        }
        opcode::SMSG_TAXINODE_STATUS => {
            let (guid, known) = taxi::read_taxi_node_status(&mut r)?;
            ServerPacket::TaxiNodeStatus {
                guid,
                known: known != 0,
            }
        }
        opcode::SMSG_ACTIVATETAXIREPLY => ServerPacket::ActivateTaxiReply {
            code: taxi::read_activate_taxi_reply(&mut r)?,
        },
        opcode::SMSG_NEW_TAXI_PATH => ServerPacket::NewTaxiPath,
        opcode::SMSG_MAIL_LIST_RESULT => ServerPacket::MailList {
            mails: mail::read_mail_list_result(&mut r)?,
        },
        opcode::SMSG_SEND_MAIL_RESULT => {
            let (mail_id, action, error, equip_error, item) = mail::read_send_mail_result(&mut r)?;
            ServerPacket::SendMailResult {
                mail_id,
                action,
                error,
                equip_error,
                item,
            }
        }
        opcode::SMSG_ITEM_TEXT_QUERY_RESPONSE => {
            let (text_id, text) = mail::read_item_text_query_response(&mut r)?;
            ServerPacket::ItemTextQueryResponse { text_id, text }
        }
        opcode::SMSG_RECEIVED_MAIL => ServerPacket::ReceivedMail {
            seconds: mail::read_received_mail(&mut r)?,
        },
        opcode::MSG_QUERY_NEXT_MAIL_TIME => ServerPacket::NextMailTime {
            seconds: mail::read_query_next_mail_time(&mut r)?,
        },
        // The three list results share one frame and a 64-byte record; the reader is bounded by
        // the buffer too, since vmangos's browse fast path can count a record it never writes.
        opcode::MSG_AUCTION_HELLO => {
            let (auctioneer, house_id) = auction::read_auction_hello(&mut r)?;
            ServerPacket::AuctionHello {
                auctioneer,
                house_id,
            }
        }
        opcode::SMSG_AUCTION_COMMAND_RESULT => {
            let (auction_id, action, error, tail) = auction::read_auction_command_result(&mut r)?;
            ServerPacket::AuctionCommandResult {
                auction_id,
                action,
                error,
                tail,
            }
        }
        opcode::SMSG_AUCTION_LIST_RESULT => {
            let (auctions, total_count) = auction::read_auction_list_result(&mut r)?;
            ServerPacket::AuctionListResult {
                auctions,
                total_count,
            }
        }
        opcode::SMSG_AUCTION_OWNER_LIST_RESULT => {
            let (auctions, total_count) = auction::read_auction_list_result(&mut r)?;
            ServerPacket::AuctionOwnerListResult {
                auctions,
                total_count,
            }
        }
        opcode::SMSG_AUCTION_BIDDER_LIST_RESULT => {
            let (auctions, total_count) = auction::read_auction_list_result(&mut r)?;
            ServerPacket::AuctionBidderListResult {
                auctions,
                total_count,
            }
        }
        // Different field orders, and no house id on the owner's: two readers, never one.
        opcode::SMSG_AUCTION_BIDDER_NOTIFICATION => ServerPacket::AuctionBidderNotification(
            auction::read_auction_bidder_notification(&mut r)?,
        ),
        opcode::SMSG_AUCTION_OWNER_NOTIFICATION => ServerPacket::AuctionOwnerNotification(
            auction::read_auction_owner_notification(&mut r)?,
        ),
        opcode::SMSG_AUCTION_REMOVED_NOTIFICATION => {
            let (auction_id, item_entry, random_property_id) =
                auction::read_auction_removed_notification(&mut r)?;
            ServerPacket::AuctionRemovedNotification {
                auction_id,
                item_entry,
                random_property_id,
            }
        }
        opcode::SMSG_TRADE_STATUS => ServerPacket::TradeStatus {
            status: trade::read_trade_status(&mut r)?,
        },
        opcode::SMSG_TRADE_STATUS_EXTENDED => ServerPacket::TradeStatusExtended {
            state: Box::new(trade::read_trade_status_extended(&mut r)?),
        },
        // The world-state table, source of the `$<n>w`/`$<n>e` NPC-text tokens.
        opcode::SMSG_INIT_WORLD_STATES => {
            ServerPacket::InitWorldStates(world_state::read_init_world_states(&mut r)?)
        }
        opcode::SMSG_UPDATE_WORLD_STATE => {
            let (id, value) = world_state::read_update_world_state(&mut r)?;
            ServerPacket::UpdateWorldState { id, value }
        }
        // Another player's move, relayed under the mover's opcode (creatures use monster moves).
        o if is_movement_relay(o) => {
            let guid = read_packed_guid(&mut r)?;
            let info = movement::read_movement_info(&mut r)?;
            ServerPacket::PlayerMove {
                guid,
                opcode: o,
                flags: info.flags,
                position: info.position,
                orientation: info.orientation,
                pitch: info.pitch,
                time: info.timestamp,
                fall_time: info.fall_time,
                jump: info.jump,
                transport: info.transport,
            }
        }
        opcode::SMSG_ADDON_INFO => ServerPacket::AddonInfo {
            statuses: read_addon_info(&mut r),
        },
        other => ServerPacket::Other { opcode: other },
    };
    *cursor = r;
    Ok(packet)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trailing_byte_is_reported_as_tail_one_and_never_as_a_failure() {
        // `SMSG_SET_FACTION_VISIBLE`: exactly one `u32 repListId`.
        let body = 21u32.to_le_bytes().to_vec();
        let (packet, tail) = parse_server_with_tail(opcode::SMSG_SET_FACTION_VISIBLE, &body)
            .expect("a well-formed body decodes");
        assert!(matches!(
            packet,
            ServerPacket::SetFactionVisible { list_id: 21 }
        ));
        assert_eq!(tail, 0);

        let mut longer = body.clone();
        longer.push(0xEE);
        let (packet, tail) = parse_server_with_tail(opcode::SMSG_SET_FACTION_VISIBLE, &longer)
            .expect("a trailing byte is not a parse failure");
        assert!(matches!(
            packet,
            ServerPacket::SetFactionVisible { list_id: 21 }
        ));
        assert_eq!(tail, 1);
        assert!(parse_server(opcode::SMSG_SET_FACTION_VISIBLE, &longer).is_ok());
    }

    #[test]
    fn the_compressed_update_object_reports_the_inflated_streams_tail_not_its_zlib_bytes() {
        use std::io::Write;
        fn body(inflated: &[u8]) -> Vec<u8> {
            let mut out = (inflated.len() as u32).to_le_bytes().to_vec();
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(inflated).unwrap();
            out.extend(z.finish().unwrap());
            out
        }
        // `u32 count = 0, u8 has_transport = 0`: a well-formed, empty object list.
        let empty = [0u8, 0, 0, 0, 0];
        let (packet, tail) =
            parse_server_with_tail(opcode::SMSG_COMPRESSED_UPDATE_OBJECT, &body(&empty))
                .expect("an empty update object decodes");
        assert!(matches!(packet, ServerPacket::UpdateObject { ref objects } if objects.is_empty()));
        assert_eq!(tail, 0, "the zlib bytes are consumed, not reported");

        let mut longer = empty.to_vec();
        longer.push(0xEE);
        let (_, tail) =
            parse_server_with_tail(opcode::SMSG_COMPRESSED_UPDATE_OBJECT, &body(&longer))
                .expect("a trailing inflated byte is not a failure");
        assert_eq!(tail, 1, "the inflated stream's leftover is the tail");

        // Bytes after the zlib stream, still in the packet body, are the outer tail.
        let mut outer = body(&empty);
        outer.extend([0xAB, 0xCD]);
        let (_, tail) = parse_server_with_tail(opcode::SMSG_COMPRESSED_UPDATE_OBJECT, &outer)
            .expect("bytes after the stream are not a failure");
        assert_eq!(
            tail, 2,
            "the body's bytes after the zlib stream are the tail"
        );
    }

    #[test]
    fn an_unknown_opcode_reports_no_tail() {
        let (packet, tail) =
            parse_server_with_tail(0xFFFF, &[1, 2, 3]).expect("unknown opcodes never fail");
        assert!(matches!(packet, ServerPacket::Other { opcode: 0xFFFF }));
        assert_eq!(tail, 0);
    }

    /// A count past the body ends in `UnexpectedEof` at the first missing row, in both opcodes.
    #[test]
    fn a_lying_faction_count_is_a_short_read() {
        let mut body = 0xFFFF_FFFFu32.to_le_bytes().to_vec();
        body.push(0x01); // flags
        body.extend_from_slice(&3000i32.to_le_bytes()); // standing
        let err = parse_server(opcode::SMSG_INITIALIZE_FACTIONS, &body)
            .err()
            .expect("the second row is missing, so the read is short");
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);

        let mut body = 0xFFFF_FFFFu32.to_le_bytes().to_vec();
        body.extend_from_slice(&5u32.to_le_bytes());
        body.extend_from_slice(&(-100i32).to_le_bytes());
        let err = parse_server(opcode::SMSG_SET_FACTION_STANDING, &body)
            .err()
            .expect("the second row is missing, so the read is short");
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
