//! The sends about being logged in rather than about the world: the keepalive, logout, the server
//! clock and the cinematic acks.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_PING`, every ~30 s: the server echoes `sequence` in `SMSG_PONG` and stores
    /// `last_rtt_ms` as our latency. vmangos kicks pings under 27 s apart, so never retry one.
    pub fn ping(&mut self, sequence: u32, last_rtt_ms: u32) -> Result<()> {
        self.send(opcode::CMSG_PING, &messages::ping(sequence, last_rtt_ms))
    }

    /// `CMSG_LOGOUT_REQUEST`, empty: answered by `SMSG_LOGOUT_RESPONSE`, then
    /// `SMSG_LOGOUT_COMPLETE` when the logout ends (at once for a resting or GM character).
    pub fn logout_request(&mut self) -> Result<()> {
        self.send(opcode::CMSG_LOGOUT_REQUEST, &[])
    }

    /// `CMSG_PLAYER_LOGOUT`, empty: the forced logout the 1.12 client's `ForceLogout()` sends.
    pub fn player_logout(&mut self) -> Result<()> {
        self.send(opcode::CMSG_PLAYER_LOGOUT, &[])
    }

    /// `CMSG_LOGOUT_CANCEL`, empty: the server stops its 20 s logout timer, unroots the character
    /// and answers `SMSG_LOGOUT_CANCEL_ACK`.
    pub fn logout_cancel(&mut self) -> Result<()> {
        self.send(opcode::CMSG_LOGOUT_CANCEL, &[])
    }

    /// `CMSG_QUERY_TIME`, empty: answered with the server's unix time as one `u32`. Descriptor
    /// deadlines, such as a timed quest's, are absolute server time, so countdowns need this clock.
    pub fn query_time(&mut self) -> Result<()> {
        self.send(opcode::CMSG_QUERY_TIME, &messages::query_time())
    }

    /// `CMSG_OPENING_CINEMATIC`, empty: Lua's `OpeningCinematic()` (`0x48c8c0`, opcode `0xf9` put
    /// at `0x48c8c9`, sent through `0x5ab630`).
    pub fn opening_cinematic(&mut self) -> Result<()> {
        self.send(opcode::CMSG_OPENING_CINEMATIC, &[])
    }

    /// `CMSG_COMPLETE_CINEMATIC`, empty: sent when a cinematic ends or is escaped, and owed for
    /// every `SMSG_TRIGGER_CINEMATIC`; until then vmangos anchors visibility to the cinematic
    /// camera and the world around the body despawns.
    pub fn complete_cinematic(&mut self) -> Result<()> {
        self.send(opcode::CMSG_COMPLETE_CINEMATIC, &[])
    }

    /// `CMSG_NEXT_CINEMATIC_CAMERA`, empty: sent as each shot is armed, the first included, just
    /// before its narration (reference: shot arm `0x48edf0`, not the advance `0x48efe0`). Every
    /// 1.12 `CinematicSequences` row has one camera, so a race intro sends exactly one.
    pub fn next_cinematic_camera(&mut self) -> Result<()> {
        self.send(opcode::CMSG_NEXT_CINEMATIC_CAMERA, &[])
    }
}
