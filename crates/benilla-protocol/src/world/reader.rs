use std::net::TcpStream;

use anyhow::{anyhow, Result};
use benilla_srp::vanilla_header::DecrypterHalf;

use crate::messages::{self, ServerPacket};

use super::{read_frame, recv_packet};

/// The read half of a split [`WorldSession`](super::WorldSession): cloned socket and decrypter,
/// and what the connection learned before the split, for
/// [`WorldSession::rejoin`](super::WorldSession::rejoin).
pub struct WorldReader {
    pub(super) stream: TcpStream,
    pub(super) decrypter: DecrypterHalf,
    pub(super) link: super::session::LinkFacts,
    pub(super) traffic: std::sync::Arc<super::LinkTraffic>,
}

impl WorldReader {
    /// Read + decrypt one server packet (blocking).
    pub fn recv(&mut self) -> Result<ServerPacket> {
        recv_packet(&mut self.stream, Some(&mut self.decrypter), &self.traffic)
    }

    /// Read one packet and decode it into a [`crate::Poll`]; errors only when the socket fails. The
    /// whole body is read before parsing, so an unparseable packet is skipped, the stream aligned.
    pub fn poll(&mut self) -> Result<crate::Poll> {
        let (opcode, body) = read_frame(&mut self.stream, Some(&mut self.decrypter), &self.traffic)
            .map_err(|e| anyhow!("world stream closed: {e}"))?;
        match messages::parse_server_with_tail(opcode, &body) {
            Ok((packet, tail)) => Ok(crate::Poll::Events {
                opcode,
                events: crate::decode(packet),
                tail,
                body,
            }),
            Err(e) => Ok(crate::Poll::Skipped {
                opcode,
                reason: format!(
                    "opcode {opcode:#06x} ({}): {e} [{}B: {}]",
                    messages::opcode_name(opcode).unwrap_or("?"),
                    body.len(),
                    hex_preview(&body, 64)
                ),
                body,
            }),
        }
    }
}

/// Hex of the first `max` bytes of `body`, `…` when truncated, for decoding a packet by hand.
fn hex_preview(body: &[u8], max: usize) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for b in body.iter().take(max) {
        let _ = write!(s, "{b:02x} ");
    }
    if body.len() > max {
        s.push('…');
    }
    s.trim_end().to_string()
}
