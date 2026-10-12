use std::io::Read;
use std::net::TcpStream;

use anyhow::{anyhow, Result};
use benilla_srp::vanilla_header::DecrypterHalf;

use crate::messages::{self, ServerPacket};

use super::recv_packet;

/// The read half of a split [`WorldSession`](super::WorldSession): cloned socket and decrypter.
pub struct WorldReader {
    pub(super) stream: TcpStream,
    pub(super) decrypter: DecrypterHalf,
}

impl WorldReader {
    /// Read + decrypt one server packet (blocking).
    pub fn recv(&mut self) -> Result<ServerPacket> {
        recv_packet(&mut self.stream, Some(&mut self.decrypter))
    }

    /// Read one packet and decode it into a [`crate::Poll`]; errors only when the socket fails. The
    /// whole body is read before parsing, so an unparseable packet is skipped, the stream aligned.
    pub fn poll(&mut self) -> Result<crate::Poll> {
        let mut header = [0u8; 4];
        if let Err(e) = self.stream.read_exact(&mut header) {
            return Err(anyhow!("world stream closed: {e}"));
        }
        self.decrypter.decrypt(&mut header);
        let size = u16::from_be_bytes([header[0], header[1]]);
        let opcode = u16::from_le_bytes([header[2], header[3]]);
        let body_len = size.saturating_sub(2) as usize;
        let mut body = vec![0u8; body_len];
        if let Err(e) = self.stream.read_exact(&mut body) {
            return Err(anyhow!("world stream closed: {e}"));
        }
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
                    "opcode {opcode:#06x} ({}): {e} [{body_len}B: {}]",
                    messages::opcode_name(opcode).unwrap_or("?"),
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
