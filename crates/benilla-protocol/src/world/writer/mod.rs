//! The world session's write half: [`WorldWriter`] and one method per thing the player can do.
//! Each family module mirrors its namesake in `crate::messages`, which builds the bodies.

use std::net::TcpStream;

use anyhow::Result;
use benilla_srp::vanilla_header::EncrypterHalf;

use super::send_packet;

mod action_bar;
mod area_trigger;
mod attack;
mod auction;
mod bank;
mod battlefield;
mod binder;
mod channel;
mod chat;
mod death;
mod duel;
mod gameobject;
mod gm_ticket;
mod gossip;
mod group;
mod guild;
mod instance;
mod items;
mod lifecycle;
mod loot;
mod mail;
mod meeting_stone;
mod names;
mod pet;
mod petition;
mod player_flags;
mod pose;
mod progression;
mod pvp;
mod quest;
mod reputation;
mod selection;
mod self_movement;
mod skills;
mod social;
mod spells;
mod stable;
mod summon;
mod tabard;
mod taxi;
mod trade;
mod trainer;
mod tutorial;
mod vendor;

/// Write half of a split [`WorldSession`](super::WorldSession): a cloned socket and the encrypter.
/// Movement sends need the active player confirmed as mover first
/// ([`WorldSession::set_active_mover`](super::WorldSession::set_active_mover)). Positions are raw
/// WoW yards; `orientation` is radians.
pub struct WorldWriter {
    pub(super) stream: TcpStream,
    pub(super) encrypter: EncrypterHalf,
    /// Every packet that reached the socket since the last drain, as `(opcode, body length)`;
    /// `None` until [`Self::watch_sends`]. Pushed only after a successful write.
    pub(super) sent: Option<Vec<(u16, usize)>>,
    /// The character's faction language, set at login from its race, which a chat send naming no
    /// language carries. vmangos drops the whole message, dot-commands included, when the
    /// character does not know the language.
    pub(super) chat_language: u32,
}

impl WorldWriter {
    /// Frame, encrypt and write one packet: the sole write path every verb goes through.
    fn send(&mut self, opcode: u16, body: &[u8]) -> Result<()> {
        let sent = send_packet(&mut self.stream, Some(&mut self.encrypter), opcode, body);
        if sent.is_ok() {
            if let Some(log) = &mut self.sent {
                log.push((opcode, body.len()));
            }
        }
        sent
    }

    /// Start recording what reaches the socket; a second call keeps what is not yet drained.
    pub fn watch_sends(&mut self) {
        self.sent.get_or_insert_with(Vec::new);
    }

    /// Hand over and clear the recorded `(opcode, body length)` pairs, in send order.
    pub fn drain_sent(&mut self, mut each: impl FnMut(u16, usize)) {
        if let Some(log) = &mut self.sent {
            for (opcode, len) in log.drain(..) {
                each(opcode, len);
            }
        }
    }
}
