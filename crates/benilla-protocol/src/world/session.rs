use std::net::{TcpStream, ToSocketAddrs};

use anyhow::{anyhow, bail, Context, Result};
use benilla_srp::vanilla_header::{HeaderCrypto, ProofSeed};
use benilla_srp::{NormalizedString, SESSION_KEY_LENGTH};

use crate::messages::{self, opcode, Character, MoveMode, ServerPacket};

use super::movement::{client_uptime_ms, movement_info, MOVEMENT_FLAG_FORWARD};
use super::reader::WorldReader;
use super::writer::WorldWriter;
use super::{recv_packet, send_packet};

/// Read timeout through `player_login`, where each step awaits one reply.
const HANDSHAKE_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Read timeout in the login queue, where updates can be minutes apart. Deviation: the reference
/// waits forever; a bound tells a server that died mid-queue from a long wait.
const QUEUE_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// The server requires Warden. Deviation: benilla does not implement it; vmangos ships with it
/// off (`Warden.WinEnabled`, `Warden.OSXEnabled`). vmangos kicks a client that leaves a Warden
/// request unanswered for 30 s (`Warden.cpp`), so the connect refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WardenRequired;

/// The world server refused the session. Its code is from `messages`' `AUTH_*` block, not
/// [`crate::AuthReject`]'s, whose numbers overlap with unrelated meanings.
#[derive(Debug, Clone, Copy)]
pub struct WorldAuthReject {
    pub code: u8,
}

impl std::fmt::Display for WorldAuthReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "world server refused the session: result {:#04x}",
            self.code
        )
    }
}

impl std::error::Error for WorldAuthReject {}

impl std::fmt::Display for WardenRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "this server requires the Warden anticheat, which benilla does not implement"
        )
    }
}

impl std::error::Error for WardenRequired {}

/// An authenticated world-server session with 1.12 header obfuscation active.
pub struct WorldSession {
    stream: TcpStream,
    crypto: HeaderCrypto,
    /// Roster guid to race, so [`Self::player_login`] can pick the chat language.
    roster_races: std::collections::HashMap<u64, u8>,
    /// The language chat sends speak, the character's faction tongue: vmangos drops chat, even
    /// dot-commands, in a language the character does not know.
    chat_language: u32,
    /// The rested billing minutes the admitting `SMSG_AUTH_RESPONSE` carried. Deviation: `0` when
    /// the body is too short, where the reference leaves its global as it was; this field lives
    /// per connection, with nothing earlier to keep.
    billing_time_rested: u32,
    /// `SMSG_TUTORIAL_FLAGS` when it lands during the handshake rather than in the world stream.
    tutorial_flags: Option<Vec<u8>>,
    /// `SMSG_ADDON_INFO`'s per-record status bytes in order; `None` until the server answers.
    addon_info: Option<Vec<u8>>,
}

impl WorldSession {
    /// Connect and complete the auth handshake with the logon's account and session key.
    pub fn connect(
        addr: impl ToSocketAddrs,
        username: &str,
        session_key: [u8; SESSION_KEY_LENGTH],
    ) -> Result<Self> {
        Self::connect_queued(addr, username, session_key, &mut |_| true)
    }

    /// [`Self::connect`], calling `on_queue` with each queue position; `false` abandons the queue.
    /// Deviation: checked per queue packet where the reference checks each tick, so a cancel can
    /// lag one server update.
    pub fn connect_queued(
        addr: impl ToSocketAddrs,
        username: &str,
        session_key: [u8; SESSION_KEY_LENGTH],
        on_queue: &mut dyn FnMut(Option<u32>) -> bool,
    ) -> Result<Self> {
        let mut queued = false;
        let mut stream = TcpStream::connect(addr).context("connecting to world server")?;
        // Nagle off: the reference sets `TCP_NODELAY` on its game socket (`0x5bca60`).
        stream
            .set_nodelay(true)
            .context("disabling Nagle (TCP_NODELAY) on the world socket")?;
        // `into_split` clears this for the streaming phase, where a quiet world is legal.
        stream
            .set_read_timeout(Some(HANDSHAKE_READ_TIMEOUT))
            .context("setting handshake read timeout")?;

        let server_seed = match recv_packet(&mut stream, None)? {
            ServerPacket::AuthChallenge { server_seed } => server_seed,
            other => bail!("expected SMSG_AUTH_CHALLENGE, got {}", other.name()),
        };

        let username_n =
            NormalizedString::new(username).map_err(|e| anyhow!("invalid username: {e}"))?;
        let seed = ProofSeed::new();
        let client_seed = seed.seed();
        let (client_proof, crypto) =
            seed.into_client_header_crypto(&username_n, session_key, server_seed);

        // Sent plain; header encryption starts right after. The addon block is required (cmangos
        // kicks a zero-size one); `STOCK_SECURE_ADDONS` is what a stock 1.12.1 install reports.
        let body = messages::auth_session(
            u32::from(crate::CLIENT_BUILD),
            &username.to_uppercase(),
            client_seed,
            &client_proof,
            &messages::STOCK_SECURE_ADDONS,
        );
        send_packet(&mut stream, None, opcode::CMSG_AUTH_SESSION, &body)
            .context("sending CMSG_AUTH_SESSION")?;

        let mut session = WorldSession {
            stream,
            crypto,
            roster_races: Default::default(),
            chat_language: messages::LANGUAGE_COMMON,
            billing_time_rested: 0,
            tutorial_flags: None,
            addon_info: None,
        };

        // AUTH_RESPONSE is not always first, so others are skipped; Warden data ends the connect.
        loop {
            match session.recv()? {
                ServerPacket::AuthResponse {
                    result,
                    billing_time_rested,
                    ..
                } if result == messages::AUTH_OK => {
                    // The reference keeps the last AUTH_RESPONSE's billing group.
                    session.billing_time_rested = billing_time_rested.unwrap_or(0);
                    break;
                }
                // Queued, not refused: the server re-sends as we move up and ends with `AUTH_OK`.
                ServerPacket::AuthResponse {
                    result,
                    queue_position,
                    ..
                } if result == messages::AUTH_WAIT_QUEUE => {
                    if !queued {
                        queued = true;
                        session
                            .set_read_timeout(Some(QUEUE_READ_TIMEOUT))
                            .context("relaxing the read timeout for the login queue")?;
                    }
                    if !on_queue(queue_position) {
                        bail!("login queue abandoned");
                    }
                }
                ServerPacket::AuthResponse { result, .. } => {
                    return Err(WorldAuthReject { code: result }.into())
                }
                ServerPacket::Other {
                    opcode: opcode::SMSG_WARDEN_DATA,
                } => return Err(WardenRequired.into()),
                _ => continue,
            }
        }
        // Admitted: the rest of the handshake is prompt again.
        if queued {
            session
                .set_read_timeout(Some(HANDSHAKE_READ_TIMEOUT))
                .context("restoring the handshake read timeout after the queue")?;
        }

        Ok(session)
    }

    /// Rested billing minutes (`GetBillingTimeRested`); always 0 from vmangos (`World.cpp:331`).
    pub fn billing_time_rested(&self) -> u32 {
        self.billing_time_rested
    }

    /// The tutorial bank captured during the login handshake, if any.
    pub fn take_tutorial_flags(&mut self) -> Option<Vec<u8>> {
        self.tutorial_flags.take()
    }

    /// Set the socket's read timeout; `None` makes reads block.
    pub fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> Result<()> {
        self.stream
            .set_read_timeout(timeout)
            .context("setting world socket read timeout")
    }

    /// Read + decrypt + parse one server packet.
    pub fn recv(&mut self) -> Result<ServerPacket> {
        let packet = recv_packet(&mut self.stream, Some(self.crypto.decrypter()))?;
        // `SMSG_ADDON_INFO` can reach any of the handshake's read loops, so it is caught here.
        if let ServerPacket::AddonInfo { statuses } = &packet {
            self.addon_info = Some(statuses.clone());
        }
        Ok(packet)
    }

    /// The `SMSG_ADDON_INFO` statuses, taken once; `None` is real, as vmangos stays silent when it
    /// rejects the addon block (`WorldSocket.cpp:447`).
    pub fn take_addon_info(&mut self) -> Option<Vec<u8>> {
        self.addon_info.take()
    }

    /// Send a client packet (encrypted header + plaintext body).
    fn send(&mut self, opcode: u16, body: &[u8]) -> Result<()> {
        send_packet(
            &mut self.stream,
            Some(self.crypto.encrypter()),
            opcode,
            body,
        )
    }

    /// Request and return the character list, remembering each race for the chat language.
    pub fn char_enum(&mut self) -> Result<Vec<Character>> {
        self.send(opcode::CMSG_CHAR_ENUM, &[])?;
        loop {
            match self.recv()? {
                ServerPacket::CharEnum { characters } => {
                    self.roster_races = characters.iter().map(|c| (c.guid, c.race)).collect();
                    return Ok(characters);
                }
                // Warden can arm on either side of SMSG_AUTH_RESPONSE, so this step refuses it too.
                ServerPacket::Other {
                    opcode: opcode::SMSG_WARDEN_DATA,
                } => return Err(WardenRequired.into()),
                // Kept for the world entry when the server sends it this early.
                ServerPacket::TutorialFlags(flags) => {
                    self.tutorial_flags = Some(flags.bytes);
                    continue;
                }
                // The server interleaves account-data / cache packets here; skip them.
                _ => continue,
            }
        }
    }

    /// Create a character; returns the `SMSG_CHAR_CREATE` result byte.
    pub fn create_character(&mut self, req: &messages::CharCreateReq) -> Result<u8> {
        self.send(opcode::CMSG_CHAR_CREATE, &messages::char_create(req))?;
        loop {
            match self.recv()? {
                ServerPacket::CharCreate { result } => return Ok(result),
                _ => continue,
            }
        }
    }

    /// Delete a character at character select; returns the `SMSG_CHAR_DELETE` result byte.
    pub fn delete_character(&mut self, guid: u64) -> Result<u8> {
        self.send(opcode::CMSG_CHAR_DELETE, &messages::full_guid(guid))?;
        loop {
            match self.recv()? {
                ServerPacket::CharDelete { result } => return Ok(result),
                _ => continue,
            }
        }
    }

    /// Enter the world as `guid`, adopting its faction tongue for chat (Common if not enumerated).
    pub fn player_login(&mut self, guid: u64) -> Result<()> {
        self.chat_language = self
            .roster_races
            .get(&guid)
            .map_or(messages::LANGUAGE_COMMON, |&race| {
                messages::faction_language(race)
            });
        self.send(opcode::CMSG_PLAYER_LOGIN, &messages::full_guid(guid))
    }

    /// Declare the unit we move, as the 1.12 client does at its own create; vmangos drops moves
    /// until then.
    pub fn set_active_mover(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_SET_ACTIVE_MOVER, &messages::full_guid(guid))
    }

    /// Acknowledge a triggered cinematic as finished (`CMSG_COMPLETE_CINEMATIC`, empty body).
    pub fn complete_cinematic(&mut self) -> Result<()> {
        self.send(opcode::CMSG_COMPLETE_CINEMATIC, &[])
    }

    /// Start walking forward (`MSG_MOVE_START_FORWARD`).
    pub fn start_forward(&mut self, pos: [f32; 3], orientation: f32) -> Result<()> {
        self.send(
            opcode::MSG_MOVE_START_FORWARD,
            &messages::movement(&movement_info(pos, orientation, MOVEMENT_FLAG_FORWARD)),
        )
    }

    /// Continue moving (`MSG_MOVE_HEARTBEAT`).
    pub fn heartbeat(&mut self, pos: [f32; 3], orientation: f32) -> Result<()> {
        self.send(
            opcode::MSG_MOVE_HEARTBEAT,
            &messages::movement(&movement_info(pos, orientation, MOVEMENT_FLAG_FORWARD)),
        )
    }

    /// Stop (`MSG_MOVE_STOP`, no movement flags).
    pub fn stop(&mut self, pos: [f32; 3], orientation: f32) -> Result<()> {
        self.send(
            opcode::MSG_MOVE_STOP,
            &messages::movement(&movement_info(pos, orientation, 0)),
        )
    }

    /// Ack a `SMSG_FORCE_*_SPEED_CHANGE` at rest with the full guid, counter and exact speed.
    pub fn force_speed_ack(
        &mut self,
        kind: messages::SpeedKind,
        guid: u64,
        counter: u32,
        speed: f32,
        pos: [f32; 3],
        orientation: f32,
    ) -> Result<()> {
        self.send(
            kind.ack_opcode(),
            &messages::force_speed_ack(guid, counter, &movement_info(pos, orientation, 0), speed),
        )
    }

    /// Ack a finished self `SMSG_MONSTER_MOVE` at its endpoint (`CMSG_MOVE_SPLINE_DONE`).
    pub fn move_spline_done(
        &mut self,
        pos: [f32; 3],
        orientation: f32,
        spline_id: u32,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_MOVE_SPLINE_DONE,
            &messages::move_spline_done(&movement_info(pos, orientation, 0), spline_id),
        )
    }

    /// Ask a player's name (`CMSG_NAME_QUERY`), answered by `SMSG_NAME_QUERY_RESPONSE`.
    pub fn name_query(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_NAME_QUERY, &messages::full_guid(guid))
    }

    /// Ask a creature template's name (`CMSG_CREATURE_QUERY`).
    pub fn creature_query(&mut self, entry: u32, guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_CREATURE_QUERY,
            &messages::creature_query(entry, guid),
        )
    }

    /// Cast a spell (`CMSG_CAST_SPELL`), `None` targeting self; answered by `SMSG_CAST_RESULT`.
    pub fn cast_spell(&mut self, spell_id: u32, target: Option<u64>) -> Result<()> {
        self.send(
            opcode::CMSG_CAST_SPELL,
            &messages::cast_spell(spell_id, target),
        )
    }

    /// Cast a spell at a ground point in world coords (`TARGET_FLAG_DEST_LOCATION`).
    pub fn cast_spell_at_dest(&mut self, spell_id: u32, dest: [f32; 3]) -> Result<()> {
        self.send(
            opcode::CMSG_CAST_SPELL,
            &messages::cast_spell_at_dest(spell_id, dest),
        )
    }

    /// Cast an OPEN_LOCK spell (e.g. 3365 Opening, 2575 Mining) at a GameObject.
    pub fn cast_spell_gameobject(&mut self, spell_id: u32, go_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_CAST_SPELL,
            &messages::cast_spell_gameobject(spell_id, go_guid),
        )
    }

    /// Ask an item template (`CMSG_ITEM_QUERY_SINGLE`).
    pub fn item_query(&mut self, entry: u32, guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_ITEM_QUERY_SINGLE,
            &messages::item_query(entry, guid),
        )
    }

    /// Use an item by bag position (`CMSG_USE_ITEM`).
    pub fn use_item(&mut self, bag_index: u8, slot: u8, spell_slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_USE_ITEM,
            &messages::use_item(
                bag_index,
                slot,
                spell_slot,
                messages::UseItemTarget::default(),
            ),
        )
    }

    /// Open an item by bag position (`CMSG_OPEN_ITEM`).
    pub fn open_item(&mut self, bag_index: u8, slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_OPEN_ITEM,
            &messages::open_item(bag_index, slot),
        )
    }

    /// Equip a bag item (`CMSG_AUTOEQUIP_ITEM`).
    pub fn auto_equip_item(&mut self, bag_index: u8, slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_AUTOEQUIP_ITEM,
            &messages::auto_equip_item(bag_index, slot),
        )
    }

    /// Swap two player-array slots (`CMSG_SWAP_INV_ITEM`).
    pub fn swap_inv_item(&mut self, src_slot: u8, dst_slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_SWAP_INV_ITEM,
            &messages::swap_inv_item(src_slot, dst_slot),
        )
    }

    /// Ask a vendor's stock (`CMSG_LIST_INVENTORY`), answered by `SMSG_LIST_INVENTORY`.
    pub fn list_inventory(&mut self, vendor_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_LIST_INVENTORY,
            &messages::list_inventory(vendor_guid),
        )
    }

    /// Buy from a vendor (`CMSG_BUY_ITEM`); `entry` is the item template, not the row's `muid`.
    pub fn buy_item(&mut self, vendor_guid: u64, entry: u32, count: u8) -> Result<()> {
        self.send(
            opcode::CMSG_BUY_ITEM,
            &messages::buy_item(vendor_guid, entry, count),
        )
    }

    /// Buy into a named container slot (`CMSG_BUY_ITEM_IN_SLOT`), the merchant cursor's drop.
    pub fn buy_item_in_slot(
        &mut self,
        vendor_guid: u64,
        entry: u32,
        bag_guid: u64,
        bag_slot: u8,
        count: u8,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_BUY_ITEM_IN_SLOT,
            &messages::buy_item_in_slot(vendor_guid, entry, bag_guid, bag_slot, count),
        )
    }

    /// Sell an item to a vendor (`CMSG_SELL_ITEM`); `count` 0 sells the whole stack.
    pub fn sell_item(&mut self, vendor_guid: u64, item_guid: u64, count: u8) -> Result<()> {
        self.send(
            opcode::CMSG_SELL_ITEM,
            &messages::sell_item(vendor_guid, item_guid, count),
        )
    }

    /// Buy a sold item back (`CMSG_BUYBACK_ITEM`); `slot` is the absolute buyback slot 69-80.
    pub fn buyback_item(&mut self, vendor_guid: u64, slot: u32) -> Result<()> {
        self.send(
            opcode::CMSG_BUYBACK_ITEM,
            &messages::buyback_item(vendor_guid, slot),
        )
    }

    /// Repair at a vendor (`CMSG_REPAIR_ITEM`); `item_guid` 0 repairs everything.
    pub fn repair_item(&mut self, vendor_guid: u64, item_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_REPAIR_ITEM,
            &messages::repair_item(vendor_guid, item_guid),
        )
    }

    /// Open the bank at a banker (`CMSG_BANKER_ACTIVATE`), answered by `SMSG_SHOW_BANK`.
    pub fn banker_activate(&mut self, banker_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_BANKER_ACTIVATE,
            &messages::banker_activate(banker_guid),
        )
    }

    /// Buy the next bank bag slot; success is silent, failure answers `SMSG_BUY_BANK_SLOT_RESULT`.
    pub fn buy_bank_slot(&mut self, banker_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_BUY_BANK_SLOT,
            &messages::buy_bank_slot(banker_guid),
        )
    }

    /// Deposit the item at `(bag, slot)` into the first free bank slot (`CMSG_AUTOBANK_ITEM`).
    pub fn autobank_item(&mut self, bag: u8, slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_AUTOBANK_ITEM,
            &messages::autobank_item(bag, slot),
        )
    }

    /// Withdraw a bank position, or deposit any other (`CMSG_AUTOSTORE_BANK_ITEM`).
    pub fn autostore_bank_item(&mut self, bag: u8, slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_AUTOSTORE_BANK_ITEM,
            &messages::autostore_bank_item(bag, slot),
        )
    }

    /// Ask an NPC's overhead `!`/`?` status (`CMSG_QUESTGIVER_STATUS_QUERY`).
    pub fn questgiver_status_query(&mut self, npc: u64) -> Result<()> {
        self.send(
            opcode::CMSG_QUESTGIVER_STATUS_QUERY,
            &messages::questgiver_status_query(npc),
        )
    }

    /// Open a questgiver dialog (`CMSG_QUESTGIVER_HELLO`), the server's gossip-hello path.
    pub fn questgiver_hello(&mut self, npc: u64) -> Result<()> {
        self.send(
            opcode::CMSG_QUESTGIVER_HELLO,
            &messages::questgiver_hello(npc),
        )
    }

    /// Ask a quest's detail panel (`CMSG_QUESTGIVER_QUERY_QUEST`).
    pub fn questgiver_query_quest(&mut self, npc: u64, quest: u32) -> Result<()> {
        self.send(
            opcode::CMSG_QUESTGIVER_QUERY_QUEST,
            &messages::questgiver_query_quest(npc, quest),
        )
    }

    /// Accept a quest (`CMSG_QUESTGIVER_ACCEPT_QUEST`); the server closes the gossip window.
    pub fn questgiver_accept_quest(&mut self, npc: u64, quest: u32) -> Result<()> {
        self.send(
            opcode::CMSG_QUESTGIVER_ACCEPT_QUEST,
            &messages::questgiver_accept_quest(npc, quest),
        )
    }

    /// Ask a quest's turn-in panel: `REQUEST_ITEMS`, or `OFFER_REWARD` when nothing is required.
    pub fn questgiver_complete_quest(&mut self, npc: u64, quest: u32) -> Result<()> {
        self.send(
            opcode::CMSG_QUESTGIVER_COMPLETE_QUEST,
            &messages::questgiver_complete_quest(npc, quest),
        )
    }

    /// Advance to the reward panel (`CMSG_QUESTGIVER_REQUEST_REWARD`).
    pub fn questgiver_request_reward(&mut self, npc: u64, quest: u32) -> Result<()> {
        self.send(
            opcode::CMSG_QUESTGIVER_REQUEST_REWARD,
            &messages::questgiver_request_reward(npc, quest),
        )
    }

    /// Choose reward index `reward` and finish the quest (`CMSG_QUESTGIVER_CHOOSE_REWARD`).
    pub fn questgiver_choose_reward(&mut self, npc: u64, quest: u32, reward: u32) -> Result<()> {
        self.send(
            opcode::CMSG_QUESTGIVER_CHOOSE_REWARD,
            &messages::questgiver_choose_reward(npc, quest, reward),
        )
    }

    /// Ask a quest's full template by id alone (`CMSG_QUEST_QUERY`).
    pub fn quest_query(&mut self, quest_id: u32) -> Result<()> {
        self.send(opcode::CMSG_QUEST_QUERY, &messages::quest_query(quest_id))
    }

    /// Ask the server's unix-seconds clock, the epoch of timed-quest deadlines (`CMSG_QUERY_TIME`).
    pub fn query_time(&mut self) -> Result<()> {
        self.send(opcode::CMSG_QUERY_TIME, &messages::query_time())
    }

    /// Abandon a quest-log slot; no reply, the server clears the `PLAYER_QUEST_LOG` fields.
    pub fn questlog_remove_quest(&mut self, slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_QUESTLOG_REMOVE_QUEST,
            &messages::questlog_remove_quest(slot),
        )
    }

    /// Send a `/say` line, GM dot-commands included, in the character's own tongue.
    pub fn send_chat(&mut self, message: &str) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat(messages::CHAT_TYPE_SAY, self.chat_language, message),
        )
    }

    /// Send a chat line on any lane; `target` is the whisper target or channel name.
    pub fn send_chat_kind(
        &mut self,
        chat_type: u32,
        target: Option<&str>,
        message: &str,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat_kind(chat_type, self.chat_language, target, message),
        )
    }

    /// Join a channel; `password` is empty for a channel that has none.
    pub fn join_channel(&mut self, name: &str, password: &str) -> Result<()> {
        self.send(
            opcode::CMSG_JOIN_CHANNEL,
            &messages::join_channel(name, password),
        )
    }

    /// Leave a channel.
    pub fn leave_channel(&mut self, name: &str) -> Result<()> {
        self.send(opcode::CMSG_LEAVE_CHANNEL, &messages::leave_channel(name))
    }

    /// Invite a player to our group by name (`CMSG_GROUP_INVITE`).
    pub fn group_invite(&mut self, member_name: &str) -> Result<()> {
        self.send(
            opcode::CMSG_GROUP_INVITE,
            &messages::group_invite(member_name),
        )
    }

    /// Accept the group invite we were just offered (`CMSG_GROUP_ACCEPT`, empty body).
    pub fn group_accept(&mut self) -> Result<()> {
        self.send(opcode::CMSG_GROUP_ACCEPT, &messages::group_accept())
    }

    /// Leave or disband our group (`CMSG_GROUP_DISBAND`, empty body).
    pub fn group_disband(&mut self) -> Result<()> {
        self.send(opcode::CMSG_GROUP_DISBAND, &messages::group_disband())
    }

    /// Send an addon message: [`messages::LANGUAGE_ADDON`] as the language, `target` the channel
    /// name on [`messages::CHAT_TYPE_CHANNEL`]. vmangos drops it unless `AddonChannel` is on and
    /// the lane allows it. Probe-only: benilla runs no third-party addons.
    pub fn send_addon_message(
        &mut self,
        chat_type: u32,
        target: Option<&str>,
        text: &str,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_MESSAGECHAT,
            &messages::messagechat_kind(chat_type, messages::LANGUAGE_ADDON, target, text),
        )
    }

    /// Echo a same-map teleport ack; without it the server freezes our movement.
    pub fn teleport_ack(&mut self, guid: u64, counter: u32) -> Result<()> {
        self.send(
            opcode::MSG_MOVE_TELEPORT_ACK,
            &messages::teleport_ack(guid, counter, client_uptime_ms()),
        )
    }

    /// Ack a cross-map worldport (empty body); without it nothing on the new map is streamed.
    pub fn worldport_ack(&mut self) -> Result<()> {
        self.send(opcode::MSG_MOVE_WORLDPORT_ACK, &[])
    }

    /// Ack a granted mover mode with the counter and current `pose`; nothing applies until then.
    pub fn move_mode_ack(
        &mut self,
        guid: u64,
        counter: u32,
        mode: MoveMode,
        apply: bool,
        flags: u32,
        pose: ([f32; 3], f32),
    ) -> Result<()> {
        let info = movement_info(pose.0, pose.1, flags);
        let trailing = mode.ack_carries_apply().then_some(apply);
        self.send(
            mode.ack_opcode(apply),
            &messages::move_flag_ack(guid, counter, &info, trailing),
        )
    }

    /// Release the spirit (`CMSG_REPOP_REQUEST`, empty body).
    pub fn repop_request(&mut self) -> Result<()> {
        self.send(opcode::CMSG_REPOP_REQUEST, &[])
    }

    /// Ask where our corpse is (`MSG_CORPSE_QUERY`, empty body), answered on the same opcode.
    pub fn corpse_query(&mut self) -> Result<()> {
        self.send(opcode::MSG_CORPSE_QUERY, &[])
    }

    /// Self-resurrect; the server casts `PLAYER_SELF_RES_SPELL` and zeroes it.
    pub fn self_res(&mut self) -> Result<()> {
        self.send(opcode::CMSG_SELF_RES, &[])
    }

    /// Take the Spirit Healer's resurrection: 50% health and a 25% durability loss.
    pub fn spirit_healer_activate(&mut self, npc: u64) -> Result<()> {
        self.send(
            opcode::CMSG_SPIRIT_HEALER_ACTIVATE,
            &messages::spirit_healer_activate(npc),
        )
    }

    /// Start melee auto-attack (`CMSG_ATTACKSWING`), echoed as `SMSG_ATTACKSTART`.
    pub fn attack_swing(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_ATTACKSWING, &messages::attack_swing(guid))
    }

    /// Set our target (`CMSG_SET_SELECTION`, full guid); the client selects before it casts.
    pub fn set_selection(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_SET_SELECTION, &messages::full_guid(guid))
    }

    /// Open a loot window (`CMSG_LOOT`), answered by `SMSG_LOOT_RESPONSE`.
    pub fn loot(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_LOOT, &messages::loot(guid))
    }

    /// Use a world GameObject; the server answers by type, or not at all.
    pub fn gameobj_use(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_GAMEOBJ_USE, &messages::gameobj_use(guid))
    }

    /// Ask a GameObject template (`CMSG_GAMEOBJECT_QUERY`).
    pub fn gameobject_query(&mut self, entry: u32, guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_GAMEOBJECT_QUERY,
            &messages::gameobject_query(entry, guid),
        )
    }

    /// Take one loot row; `loot_slot` is the 0-based row of the `SMSG_LOOT_RESPONSE`.
    pub fn autostore_loot_item(&mut self, loot_slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_AUTOSTORE_LOOT_ITEM,
            &messages::autostore_loot_item(loot_slot),
        )
    }

    /// Take the loot's coin (`CMSG_LOOT_MONEY`, empty body).
    pub fn loot_money(&mut self) -> Result<()> {
        self.send(opcode::CMSG_LOOT_MONEY, &messages::loot_money())
    }

    /// Close the loot window; the server ignores `guid` and releases its own loot target.
    pub fn loot_release(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_LOOT_RELEASE, &messages::loot_release(guid))
    }

    /// Split into a reader and a writer on cloned sockets, after [`Self::player_login`].
    pub fn into_split(self) -> Result<(WorldReader, WorldWriter)> {
        // A quiet world is legal, so streaming reads block without a timeout.
        self.stream
            .set_read_timeout(None)
            .context("clearing handshake read timeout")?;
        let read_stream = self
            .stream
            .try_clone()
            .context("cloning world socket for split")?;
        let (encrypter, decrypter) = self.crypto.split();
        Ok((
            WorldReader {
                stream: read_stream,
                decrypter,
            },
            WorldWriter {
                stream: self.stream,
                encrypter,
                chat_language: self.chat_language,
            },
        ))
    }

    /// Log out to character select, waiting up to `timeout`; needs a socket read timeout.
    pub fn logout(&mut self, timeout: std::time::Duration) -> Result<()> {
        self.send(opcode::CMSG_LOGOUT_REQUEST, &[])?;
        let deadline = std::time::Instant::now() + timeout;
        let mut denied = false;
        while std::time::Instant::now() < deadline {
            match self.recv() {
                Ok(ServerPacket::LogoutComplete) => return Ok(()),
                Ok(ServerPacket::LogoutResponse { reason, .. }) => {
                    // A non-zero reason means the server refused (combat, falling, GM-frozen).
                    denied = reason != messages::LOGOUT_SUCCESS;
                }
                Ok(_) => {}
                Err(_) => {} // a read-timeout tick; poll until the deadline
            }
        }
        if denied {
            bail!("logout refused by server (in combat / not allowed)")
        }
        bail!("timed out waiting for SMSG_LOGOUT_COMPLETE")
    }
}
