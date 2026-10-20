//! The world server (`mangosd`) connection, opened with the realm logon's SRP6 session key.
//!
//! `SMSG_AUTH_CHALLENGE` (server seed) and `CMSG_AUTH_SESSION` (build, uppercased account, client
//! seed, SHA1 over key and seeds) travel plain; every header after them is encrypted (4-byte
//! server, 6-byte client), bodies never, starting with `SMSG_AUTH_RESPONSE`.

use std::io::{Read, Write};
use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use benilla_srp::vanilla_header::{DecrypterHalf, EncrypterHalf};

use crate::messages::{self, ServerPacket};

mod movement;
mod reader;
mod session;
mod writer;

pub use reader::WorldReader;
pub use session::{WorldAuthReject, WorldConnectEvent, WorldSession};

pub use writer::WorldWriter;

/// The stock `mangosd` port, for probes that dial the world server without a realm list.
pub const WORLD_PORT: u16 = 8085;

/// One frame off the world socket: its opcode and body, the header decrypted when `decrypter` is
/// set (`None` only for `SMSG_AUTH_CHALLENGE`). The size header counts the 2-byte opcode and the
/// body, big-endian; the opcode is little-endian.
pub(super) fn read_frame(
    r: &mut impl Read,
    mut decrypter: Option<&mut DecrypterHalf>,
) -> Result<(u16, Vec<u8>)> {
    let mut header = [0u8; 4];
    read_header_bytes(r, &mut decrypter, &mut header)
        .map_err(|e| anyhow!("reading world header: {e}"))?;
    let size = usize::from(u16::from_be_bytes([header[0], header[1]]));
    let opcode = u16::from_le_bytes([header[2], header[3]]);
    let body_len = size.saturating_sub(2);
    let mut body = vec![0u8; body_len];
    r.read_exact(&mut body)
        .map_err(|e| anyhow!("reading world body (opcode {opcode:#x}, {body_len} bytes): {e}"))?;
    Ok((opcode, body))
}

/// Read `buf.len()` header bytes and decrypt them in place, the cipher running byte by byte.
fn read_header_bytes(
    r: &mut impl Read,
    decrypter: &mut Option<&mut DecrypterHalf>,
    buf: &mut [u8],
) -> std::io::Result<()> {
    r.read_exact(buf)?;
    if let Some(d) = decrypter.as_deref_mut() {
        d.decrypt(buf);
    }
    Ok(())
}

/// Read, decrypt and parse one packet; `decrypter` is `None` only for `SMSG_AUTH_CHALLENGE`.
pub(super) fn recv_packet(
    stream: &mut impl Read,
    decrypter: Option<&mut DecrypterHalf>,
) -> Result<ServerPacket> {
    let (opcode, body) = read_frame(stream, decrypter)?;
    messages::parse_server(opcode, &body).map_err(|e| {
        Unparsed {
            opcode,
            reason: e.to_string(),
        }
        .into()
    })
}

/// A packet whose body did not parse. The whole body was read, so the stream is still aligned and
/// a reader may skip it and go on, as [`WorldReader::poll`] does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unparsed {
    pub opcode: u16,
    pub reason: String,
}

impl std::fmt::Display for Unparsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "parsing opcode {:#x}: {}", self.opcode, self.reason)
    }
}

impl std::error::Error for Unparsed {}

/// What [`observe_sends`] takes: called with a sent packet's opcode and its body, the bytes after
/// the 6-byte header and as plain as the caller built them (encryption covers the header alone).
pub type SendObserver = fn(opcode: u16, body: &[u8]);

static SEND_OBSERVER: OnceLock<SendObserver> = OnceLock::new();

/// Report every client packet that reaches a world socket, from the `CMSG_AUTH_SESSION` of the
/// handshake through each [`WorldSession`] and [`WorldWriter`] send, on the thread that wrote it
/// and right after the write. The first observer set is kept for the process; with none set a send
/// pays one atomic load.
pub fn observe_sends(observer: SendObserver) {
    let _ = SEND_OBSERVER.set(observer);
}

/// A client header: the size, counting the 4-byte opcode and the body, big-endian, then the
/// opcode widened to four bytes little-endian. Returns the header and its length.
fn client_header(opcode: u16, body_len: usize) -> ([u8; 7], usize) {
    let size = (body_len + 4) as u16;
    let mut header = [0u8; 7];
    header[..2].copy_from_slice(&size.to_be_bytes());
    header[2..6].copy_from_slice(&u32::from(opcode).to_le_bytes());
    (header, 6)
}

/// Write one client packet: its header ([`client_header`]), encrypted when `encrypter` is set,
/// then the body. The sole write path of the world socket, so the one place [`observe_sends`]
/// reports from.
pub(super) fn send_packet(
    stream: &mut impl Write,
    encrypter: Option<&mut EncrypterHalf>,
    opcode: u16,
    body: &[u8],
) -> Result<()> {
    let (mut header, header_len) = client_header(opcode, body.len());
    let header = &mut header[..header_len];
    if let Some(e) = encrypter {
        e.encrypt(header);
    }
    // One write, not two: header and body leave as one segment.
    let mut packet = Vec::with_capacity(header.len() + body.len());
    packet.extend_from_slice(header);
    packet.extend_from_slice(body);
    stream
        .write_all(&packet)
        .map_err(|e| anyhow!("sending opcode {opcode:#x}: {e}"))?;
    if let Some(observer) = SEND_OBSERVER.get() {
        observer(opcode, body);
    }
    Ok(())
}
