//! The world server (`mangosd`) connection, opened with the realm logon's SRP6 session key.
//!
//! `SMSG_AUTH_CHALLENGE` (server seed) and `CMSG_AUTH_SESSION` (build, uppercased account, client
//! seed, SHA1 over key and seeds) travel plain; every header after them is encrypted (4-byte
//! server, 6-byte client), bodies never, starting with `SMSG_AUTH_RESPONSE`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use benilla_srp::vanilla_header::{DecrypterHalf, EncrypterHalf};

use crate::messages::{self, ServerPacket};

mod movement;
mod reader;
mod session;
mod writer;

pub use reader::WorldReader;
pub use session::{WardenRequired, WorldAuthReject, WorldSession};
pub use writer::WorldWriter;

/// The stock `mangosd` port, for probes that dial the world server without a realm list.
pub const WORLD_PORT: u16 = 8085;

/// Read, decrypt and parse one packet; `decrypter` is `None` only for `SMSG_AUTH_CHALLENGE`.
pub(super) fn recv_packet(
    stream: &mut TcpStream,
    decrypter: Option<&mut DecrypterHalf>,
) -> Result<ServerPacket> {
    let mut header = [0u8; 4];
    stream
        .read_exact(&mut header)
        .map_err(|e| anyhow!("reading world header: {e}"))?;
    if let Some(d) = decrypter {
        d.decrypt(&mut header);
    }
    // size is big-endian and counts the opcode (2) but not the size field; opcode is little-endian.
    let size = u16::from_be_bytes([header[0], header[1]]);
    let opcode = u16::from_le_bytes([header[2], header[3]]);
    let body_len = size.saturating_sub(2) as usize;
    let mut body = vec![0u8; body_len];
    stream
        .read_exact(&mut body)
        .map_err(|e| anyhow!("reading world body (opcode {opcode:#x}, {body_len} bytes): {e}"))?;
    messages::parse_server(opcode, &body).map_err(|e| anyhow!("parsing opcode {opcode:#x}: {e}"))
}

/// What [`observe_sends`] takes: called with a sent packet's opcode and body length.
pub type SendObserver = fn(opcode: u16, body_len: usize);

static SEND_OBSERVER: OnceLock<SendObserver> = OnceLock::new();

/// Report every client packet that reaches a world socket, from the `CMSG_AUTH_SESSION` of the
/// handshake through each [`WorldSession`] and [`WorldWriter`] send, on the thread that wrote it
/// and right after the write. The first observer set is kept for the process; with none set a send
/// pays one atomic load.
pub fn observe_sends(observer: SendObserver) {
    let _ = SEND_OBSERVER.set(observer);
}

/// Write one client packet: a 6-byte header, its size counting opcode and body, then the body.
/// The sole write path of the world socket, so the one place [`observe_sends`] reports from.
pub(super) fn send_packet(
    stream: &mut TcpStream,
    encrypter: Option<&mut EncrypterHalf>,
    opcode: u16,
    body: &[u8],
) -> Result<()> {
    let size = (body.len() + 4) as u16;
    let mut header = {
        let s = size.to_be_bytes();
        // The client header's opcode field is 4 bytes wide; widen the 16-bit opcode to fill it.
        let o = u32::from(opcode).to_le_bytes();
        [s[0], s[1], o[0], o[1], o[2], o[3]]
    };
    if let Some(e) = encrypter {
        e.encrypt(&mut header);
    }
    // One write, not two: header and body leave as one segment.
    let mut packet = Vec::with_capacity(header.len() + body.len());
    packet.extend_from_slice(&header);
    packet.extend_from_slice(body);
    stream
        .write_all(&packet)
        .map_err(|e| anyhow!("sending opcode {opcode:#x}: {e}"))?;
    if let Some(observer) = SEND_OBSERVER.get() {
        observer(opcode, body.len());
    }
    Ok(())
}
