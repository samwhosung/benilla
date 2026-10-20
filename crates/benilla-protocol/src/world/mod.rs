//! The world server (`mangosd`) connection, opened with the realm logon's SRP6 session key.
//!
//! `SMSG_AUTH_CHALLENGE` (server seed) and `CMSG_AUTH_SESSION` (build, uppercased account, client
//! seed, SHA1 over key and seeds) travel plain; every header after them is encrypted, bodies
//! never, starting with `SMSG_AUTH_RESPONSE`. A header is the size, two bytes or three, then the
//! opcode, two bytes from the server and four from the client.

use std::io::{Read, Write};
use std::sync::OnceLock;

use anyhow::{anyhow, bail, Result};
use benilla_srp::vanilla_header::{DecrypterHalf, EncrypterHalf};

use crate::messages::{self, ServerPacket};

mod movement;
mod reader;
mod session;
mod traffic;
mod writer;

pub use reader::WorldReader;
pub use session::{WorldAuthReject, WorldConnectEvent, WorldSession};
pub use traffic::LinkTraffic;

pub use writer::WorldWriter;

/// The stock `mangosd` port, for probes that dial the world server without a realm list.
pub const WORLD_PORT: u16 = 8085;

/// The largest size the two-byte size header holds; a larger one takes three bytes, the first
/// with `0x80` set (`0x5b56da` `cmp eax,0x7fff; jg` on send, `0x5b674d` `test al,al; jns` on
/// receive).
const SHORT_SIZE_MAX: usize = 0x7fff;
/// The largest size the three-byte form holds: the first byte's low seven bits and two more.
const LONG_SIZE_MAX: usize = 0x7f_ffff;

/// One frame off the world socket: its opcode and body, the header decrypted when `decrypter` is
/// set (`None` only for `SMSG_AUTH_CHALLENGE`). The size counts the 2-byte opcode and the body.
pub(super) fn read_frame(
    r: &mut impl Read,
    mut decrypter: Option<&mut DecrypterHalf>,
    traffic: &LinkTraffic,
) -> Result<(u16, Vec<u8>)> {
    let (size, opcode) =
        read_header(r, &mut decrypter).map_err(|e| anyhow!("reading world header: {e}"))?;
    let body_len = size.saturating_sub(2);
    let mut body = vec![0u8; body_len];
    r.read_exact(&mut body)
        .map_err(|e| anyhow!("reading world body (opcode {opcode:#x}, {body_len} bytes): {e}"))?;
    traffic.add_received(size);
    Ok((opcode, body))
}

/// A server header's size and opcode. The size is big-endian, `(b0 << 8 | b1)` while the first
/// byte's top bit is clear (`0x5b677b`), else `((b0 & 0x7f) << 16 | b1 << 8 | b2)`
/// (`0x5b6765`-`0x5b6772`); the opcode after it is little-endian. The cipher runs a byte at a
/// time over both (`0x5b6848`), so the first size byte is deciphered before the size's length is
/// known, as the receive loop `0x5b65f0` reads it.
fn read_header(
    r: &mut impl Read,
    decrypter: &mut Option<&mut DecrypterHalf>,
) -> std::io::Result<(usize, u16)> {
    let mut size = [0u8; 3];
    read_header_bytes(r, decrypter, &mut size[..2])?;
    let size = if size[0] & 0x80 == 0 {
        usize::from(size[0]) << 8 | usize::from(size[1])
    } else {
        read_header_bytes(r, decrypter, &mut size[2..])?;
        usize::from(size[0] & 0x7f) << 16 | usize::from(size[1]) << 8 | usize::from(size[2])
    };
    let mut opcode = [0u8; 2];
    read_header_bytes(r, decrypter, &mut opcode)?;
    Ok((size, u16::from_le_bytes(opcode)))
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
    traffic: &LinkTraffic,
) -> Result<ServerPacket> {
    let (opcode, body) = read_frame(stream, decrypter, traffic)?;
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
/// the header and as plain as the caller built them (encryption covers the header alone).
pub type SendObserver = fn(opcode: u16, body: &[u8]);

static SEND_OBSERVER: OnceLock<SendObserver> = OnceLock::new();

/// Report every client packet that reaches a world socket, from the `CMSG_AUTH_SESSION` of the
/// handshake through each [`WorldSession`] and [`WorldWriter`] send, on the thread that wrote it
/// and right after the write. The first observer set is kept for the process; with none set a send
/// pays one atomic load.
pub fn observe_sends(observer: SendObserver) {
    let _ = SEND_OBSERVER.set(observer);
}

/// A client header and its length: the size, counting the 4-byte opcode and the body, big-endian
/// in two bytes up to [`SHORT_SIZE_MAX`], else in three, `(N >> 16) | 0x80, N >> 8, N`
/// (`0x5b5712`-`0x5b572b`); then the opcode widened to four bytes little-endian. `None` past
/// [`LONG_SIZE_MAX`]. Deviation: a size past 23 bits is refused, where the reference sends it with
/// bit 23 folded into the flag (`0x5b5712` `or dl,0x80` keeps one byte), because no client packet
/// comes near 8 MB and a refusal beats a corrupt header.
fn client_header(opcode: u16, body_len: usize) -> Option<([u8; 7], usize)> {
    let size = body_len + 4;
    let mut header = [0u8; 7];
    let size_len = if size <= SHORT_SIZE_MAX {
        header[..2].copy_from_slice(&(size as u16).to_be_bytes());
        2
    } else if size <= LONG_SIZE_MAX {
        header[..3].copy_from_slice(&(size as u32 | 0x80_0000).to_be_bytes()[1..]);
        3
    } else {
        return None;
    };
    header[size_len..size_len + 4].copy_from_slice(&u32::from(opcode).to_le_bytes());
    Some((header, size_len + 4))
}

/// Write one client packet: its header ([`client_header`]), encrypted when `encrypter` is set, as
/// the cipher covers the size and the opcode (`0x5b5842`), then the body. The sole write path of
/// the world socket, so the one place [`observe_sends`] reports from.
pub(super) fn send_packet(
    stream: &mut impl Write,
    encrypter: Option<&mut EncrypterHalf>,
    traffic: &LinkTraffic,
    opcode: u16,
    body: &[u8],
) -> Result<()> {
    let Some((mut header, header_len)) = client_header(opcode, body.len()) else {
        bail!(
            "sending opcode {opcode:#x}: {} body bytes is past the largest size header",
            body.len()
        );
    };
    let header = &mut header[..header_len];
    if let Some(e) = encrypter {
        e.encrypt(header);
    }
    // One write, not two: header and body leave as one segment.
    let mut packet = Vec::with_capacity(header.len() + body.len());
    packet.extend_from_slice(header);
    packet.extend_from_slice(body);
    // Counted whatever the socket does with it, as `0x5379c6` adds after the send returns.
    traffic.add_sent(body.len() + 4);
    stream
        .write_all(&packet)
        .map_err(|e| anyhow!("sending opcode {opcode:#x}: {e}"))?;
    if let Some(observer) = SEND_OBSERVER.get() {
        observer(opcode, body);
    }
    Ok(())
}

#[cfg(test)]
mod frame_tests {
    use benilla_srp::vanilla_header::HeaderCrypto;
    use benilla_srp::SESSION_KEY_LENGTH;

    use super::{read_frame, send_packet, LinkTraffic};

    const KEY: [u8; SESSION_KEY_LENGTH] = [0x5a; SESSION_KEY_LENGTH];
    const SMSG_UPDATE_OBJECT: u16 = 0x00a9;
    const SMSG_PONG: u16 = 0x01dd;

    /// A server frame as the wire carries it: `header` (size bytes, then the 2-byte opcode)
    /// through the server's cipher, then the body in clear.
    fn server_frame(crypto: &mut HeaderCrypto, header: &[u8], body: &[u8]) -> Vec<u8> {
        let mut header = header.to_vec();
        crypto.encrypter().encrypt(&mut header);
        header.extend_from_slice(body);
        header
    }

    fn body(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7) as u8).collect()
    }

    /// A size above `0x7fff` takes three header bytes, the first with `0x80` set
    /// (`0x5b674d` `test al,al; jns`; `0x5b6765`): `0x8001` is `80 80 01`, the opcode after it,
    /// and the next frame's two-byte header is read in step behind it.
    #[test]
    fn a_three_byte_size_header_is_read_and_the_stream_stays_in_step() {
        let mut server = HeaderCrypto::from_session_key(KEY);
        let big = body(0x7fff);
        let mut wire = server_frame(&mut server, &[0x80, 0x80, 0x01, 0xa9, 0x00], &big);
        wire.extend(server_frame(
            &mut server,
            &[0x00, 0x05, 0xdd, 0x01],
            &[1, 2, 3],
        ));

        let mut client = HeaderCrypto::from_session_key(KEY);
        let traffic = LinkTraffic::new();
        let mut r = wire.as_slice();
        let (opcode, got) = read_frame(&mut r, Some(client.decrypter()), &traffic).unwrap();
        assert_eq!((opcode, got.len()), (SMSG_UPDATE_OBJECT, 0x7fff));
        assert_eq!(got, big);
        let (opcode, got) = read_frame(&mut r, Some(client.decrypter()), &traffic).unwrap();
        assert_eq!((opcode, got), (SMSG_PONG, vec![1, 2, 3]));
        assert!(r.is_empty());
    }

    /// In clear the same bytes decode the same way: `81 23 45` is `0x12345`, the largest
    /// two-byte size `7f ff` stays two bytes (`0x5b677b`).
    #[test]
    fn the_size_forms_decode_byte_exact_in_clear() {
        let traffic = LinkTraffic::new();
        let mut wire = vec![0x81, 0x23, 0x45, 0xa9, 0x00];
        wire.extend(body(0x12343));
        wire.extend([0x7f, 0xff, 0xdd, 0x01]);
        wire.extend(body(0x7ffd));
        let mut r = wire.as_slice();
        let (opcode, got) = read_frame(&mut r, None, &traffic).unwrap();
        assert_eq!((opcode, got.len()), (SMSG_UPDATE_OBJECT, 0x12343));
        let (opcode, got) = read_frame(&mut r, None, &traffic).unwrap();
        assert_eq!((opcode, got.len()), (SMSG_PONG, 0x7ffd));
        assert!(r.is_empty());
    }

    /// The client's header: the size counts the four opcode bytes and the body, two bytes up to
    /// `0x7fff`, else `(N >> 16) | 0x80, N >> 8, N` (`0x5b56da` `cmp eax,0x7fff; jg`,
    /// `0x5b5712`-`0x5b572b`), then the opcode as four bytes little-endian.
    #[test]
    fn a_size_above_0x7fff_is_sent_in_three_bytes() {
        let traffic = LinkTraffic::new();
        let sent = |len: usize| {
            let mut wire = Vec::new();
            send_packet(&mut wire, None, &traffic, 0x01ed, &body(len)).unwrap();
            assert_eq!(&wire[wire.len() - len..], body(len).as_slice());
            wire.truncate(wire.len() - len);
            wire
        };
        assert_eq!(sent(0x7ffb), [0x7f, 0xff, 0xed, 0x01, 0x00, 0x00]);
        assert_eq!(sent(0x7ffc), [0x80, 0x80, 0x00, 0xed, 0x01, 0x00, 0x00]);
        assert_eq!(sent(0x12341), [0x81, 0x23, 0x45, 0xed, 0x01, 0x00, 0x00]);
    }

    /// The cipher covers the size header and the opcode, `h + 4` bytes (`0x5b5842`), so after a
    /// three-byte header the next packet's header is still read in step by the server.
    #[test]
    fn the_cipher_runs_over_the_three_byte_header_and_stays_in_step() {
        let traffic = LinkTraffic::new();
        let mut client = HeaderCrypto::from_session_key(KEY);
        let mut wire = Vec::new();
        send_packet(
            &mut wire,
            Some(client.encrypter()),
            &traffic,
            0x01ed,
            &body(0x8000),
        )
        .unwrap();
        send_packet(
            &mut wire,
            Some(client.encrypter()),
            &traffic,
            0x01dc,
            &[9; 8],
        )
        .unwrap();

        let mut server = HeaderCrypto::from_session_key(KEY);
        let mut first = wire[..7].to_vec();
        server.decrypter().decrypt(&mut first);
        assert_eq!(first, [0x80, 0x80, 0x04, 0xed, 0x01, 0x00, 0x00]);
        let second_at = 7 + 0x8000;
        let mut second = wire[second_at..second_at + 6].to_vec();
        server.decrypter().decrypt(&mut second);
        assert_eq!(second, [0x00, 0x0c, 0xdc, 0x01, 0x00, 0x00]);
        assert_eq!(&wire[second_at + 6..], &[9; 8]);
    }

    /// The three-byte form carries 23 bits; a bigger packet has no header, so it is refused.
    #[test]
    fn a_packet_past_23_bits_of_size_is_refused() {
        let traffic = LinkTraffic::new();
        let mut wire = Vec::new();
        assert!(send_packet(&mut wire, None, &traffic, 0x01ed, &vec![0; 0x7f_fffc]).is_err());
        assert!(wire.is_empty());
        assert!(send_packet(&mut wire, None, &traffic, 0x01ed, &vec![0; 0x7f_fffb]).is_ok());
    }

    /// `GetNetStats`' counts: a received packet adds its opcode and body (`0x537b34`), a sent one
    /// its four opcode bytes and body (`0x5379c6`); no size header counts.
    #[test]
    fn each_packet_counts_its_opcode_and_body_bytes() {
        let traffic = LinkTraffic::new();
        let mut wire = vec![0x00, 0x05, 0xdd, 0x01, 1, 2, 3];
        wire.extend([0x80, 0x80, 0x01, 0xa9, 0x00]);
        wire.extend(body(0x7fff));
        let mut r = wire.as_slice();
        read_frame(&mut r, None, &traffic).unwrap();
        assert_eq!(traffic.received(), 5);
        read_frame(&mut r, None, &traffic).unwrap();
        assert_eq!(traffic.received(), 5 + 0x8001);

        let mut out = Vec::new();
        send_packet(&mut out, None, &traffic, 0x01dc, &[0; 8]).unwrap();
        assert_eq!(traffic.sent(), 12);
        send_packet(&mut out, None, &traffic, 0x01ed, &body(0x8000)).unwrap();
        assert_eq!(traffic.sent(), 12 + 0x8004);
        assert_eq!(
            traffic.received(),
            5 + 0x8001,
            "a send counts nothing received"
        );
    }
}
