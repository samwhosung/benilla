//! The packet trace's lines: every world packet by opcode name with its length and its body as
//! hex, under the `in` and `out` tags of `WOW_MOVE_TRACE` (`benilla_assets::trace`).
//!
//! The body is the bytes after the 6-byte header, as they cross the socket (the header alone is
//! encrypted), so a line is a packet sniffer's row: paste the hex into a decoder. A field is
//! appended to each line and no earlier one moves, so a reader that matches the opcode, the name,
//! `ev=` or `len=` keeps working. A client packet that carries a secret has its bytes withheld:
//! [`withheld`].

use benilla_protocol::messages::{self, opcode};

/// What stands in for the hex of a packet that carries a secret.
const WITHHELD: &str = "<redacted>";

/// A client packet whose body is not traced: `CMSG_AUTH_SESSION` carries the account name and the
/// proof built from the session key; `CMSG_JOIN_CHANNEL` and `CMSG_CHANNEL_PASSWORD` carry a
/// channel's password; and a `CMSG_MESSAGECHAT` that types a server account command, such as
/// `.account password <old> <new>`, carries the account's. Server packets carry none.
fn withheld(opcode: u16, body: &[u8]) -> bool {
    match opcode {
        opcode::CMSG_AUTH_SESSION | opcode::CMSG_JOIN_CHANNEL | opcode::CMSG_CHANNEL_PASSWORD => {
            true
        }
        opcode::CMSG_MESSAGECHAT => {
            let text = body.to_ascii_lowercase();
            [b".account".as_slice(), b"password"]
                .iter()
                .any(|needle| text.windows(needle.len()).any(|w| w == *needle))
        }
        _ => false,
    }
}

/// `body` as lowercase hex with no separators; empty for an empty body.
fn hex(body: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(body.len() * 2);
    for &b in body {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0xf)]));
    }
    out
}

fn name(opcode: u16) -> &'static str {
    messages::opcode_name(opcode).unwrap_or("?")
}

/// The `out` line of a client packet: `0x0095 CMSG_MESSAGECHAT len=12 hex=…`.
pub(super) fn out_line(opcode: u16, body: &[u8]) -> String {
    let payload = if withheld(opcode, body) {
        WITHHELD.to_string()
    } else {
        hex(body)
    };
    format!(
        "{opcode:#06x} {} len={} hex={payload}",
        name(opcode),
        body.len()
    )
}

/// The `in` line of a server packet, decoded into `events` events: `0x0060 SMSG_… ev=1 len=4
/// hex=…`.
pub(super) fn in_line(opcode: u16, events: usize, body: &[u8]) -> String {
    format!(
        "{opcode:#06x} {} ev={events} len={} hex={}",
        name(opcode),
        body.len(),
        hex(body)
    )
}

/// The `skip` line of a server packet that did not parse: its `reason` and then its whole body,
/// which the `in` line never shows.
pub(super) fn skip_line(opcode: u16, reason: &str, body: &[u8]) -> String {
    format!("opcode={opcode:#06x} {reason} hex={}", hex(body))
}

/// The hook [`super::spawn_net`] installs for the `out` tag: one line per client packet written
/// to the world socket, whichever session or writer sent it, right after the write.
pub(super) fn trace_out(opcode: u16, body: &[u8]) {
    benilla_assets::trace::line("out", &out_line(opcode, body));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A traced packet's line carries its name, its length and its bytes, after the fields every
    /// earlier trace had.
    #[test]
    fn a_traced_packet_line_carries_its_bytes() {
        let body = [0x00, 0x0a, 0xff, 0x7e];
        assert_eq!(
            out_line(opcode::CMSG_PING, &body),
            "0x01dc CMSG_PING len=4 hex=000aff7e"
        );
        assert_eq!(
            in_line(opcode::SMSG_PONG, 1, &body),
            "0x01dd SMSG_PONG ev=1 len=4 hex=000aff7e"
        );
        assert_eq!(
            in_line(opcode::SMSG_PONG, 0, &[]),
            "0x01dd SMSG_PONG ev=0 len=0 hex=",
            "an empty body is an empty field"
        );
        assert_eq!(
            skip_line(opcode::SMSG_PONG, "short read", &body),
            "opcode=0x01dd short read hex=000aff7e"
        );
    }

    /// The bytes of a login or a password never reach the file; ordinary packets, chat included,
    /// are traced whole.
    #[test]
    fn a_packet_that_carries_a_secret_withholds_its_bytes() {
        let secret = b"SECRETWORD";
        let needle = hex(secret);
        for op in [
            opcode::CMSG_AUTH_SESSION,
            opcode::CMSG_JOIN_CHANNEL,
            opcode::CMSG_CHANNEL_PASSWORD,
        ] {
            let line = out_line(op, secret);
            assert!(line.ends_with("hex=<redacted>"), "{line}");
            assert!(!line.contains(&needle), "{line}");
        }
        // The length stays: it says the packet went out.
        assert!(out_line(opcode::CMSG_AUTH_SESSION, secret).contains("len=10"));

        // Chat: type, language, then the text.
        let chat = |text: &str| {
            let mut b = vec![0, 0, 0, 0, 0, 0, 0, 0];
            b.extend_from_slice(text.as_bytes());
            b.push(0);
            b
        };
        for command in [
            ".account password old new new",
            ".ACCOUNT create a b",
            "my Password",
        ] {
            let line = out_line(opcode::CMSG_MESSAGECHAT, &chat(command));
            assert!(line.ends_with("hex=<redacted>"), "{command}: {line}");
        }
        let hello = chat("hello there");
        let line = out_line(opcode::CMSG_MESSAGECHAT, &hello);
        assert!(line.ends_with(&format!("hex={}", hex(&hello))), "{line}");

        // Inbound traffic is never withheld.
        assert!(in_line(opcode::SMSG_AUTH_RESPONSE, 1, secret).contains(&needle));
    }
}
