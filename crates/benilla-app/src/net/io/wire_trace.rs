//! The packet trace's lines: every world packet by opcode name with its length and its body as
//! hex, under the `in` and `out` tags of `WOW_MOVE_TRACE` (`benilla_assets::trace`).
//!
//! The body is the bytes after the header, as they cross the socket (the header alone is
//! encrypted), so a line is a packet sniffer's row: paste the hex into a decoder. A field is
//! appended to each line and no earlier one moves, so a reader that matches the opcode, the name,
//! `ev=` or `len=` keeps working.
//!
//! Traces get attached to public issues, so two kinds of body never reach the file by default.
//! A packet that carries a credential is `hex=<redacted>` always ([`secret`]). Private
//! conversation and bulk lists of other players' names are `hex=<withheld>` unless
//! `WOW_MOVE_TRACE_CHAT=1` asks for them ([`private`]). Both keep `len=`.

use benilla_protocol::messages::{self, opcode};

/// What stands in for the hex of a packet that carries a secret, whatever the switches.
const REDACTED: &str = "<redacted>";
/// What stands in for the hex of private text or a list of names, until `WOW_MOVE_TRACE_CHAT=1`.
const WITHHELD: &str = "<withheld>";

/// Whether `WOW_MOVE_TRACE_CHAT` asks for the private bodies; read once.
fn reveal_private() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var_os("WOW_MOVE_TRACE_CHAT").is_some_and(|v| !v.is_empty() && v != "0")
    })
}

/// A body that is private in either direction: every chat message, sent (`CMSG_MESSAGECHAT`) or
/// received (`SMSG_MESSAGECHAT`, whispers and addon messages included), and the bulk lists of
/// other players' names, `SMSG_WHO`, `SMSG_FRIEND_LIST`, `SMSG_IGNORE_LIST`, `SMSG_GUILD_ROSTER`
/// and `SMSG_CHANNEL_LIST`. A single name (a name query, a friend's status) stays traced: it is
/// everywhere and says little alone.
fn private(opcode: u16) -> bool {
    matches!(
        opcode,
        opcode::CMSG_MESSAGECHAT
            | opcode::SMSG_MESSAGECHAT
            | opcode::SMSG_WHO
            | opcode::SMSG_FRIEND_LIST
            | opcode::SMSG_IGNORE_LIST
            | opcode::SMSG_GUILD_ROSTER
            | opcode::SMSG_CHANNEL_LIST
    )
}

/// A client packet whose body is never traced: `CMSG_AUTH_SESSION` carries the account name and
/// the proof built from the session key; `CMSG_JOIN_CHANNEL` and `CMSG_CHANNEL_PASSWORD` carry a
/// channel's password; and a `CMSG_MESSAGECHAT` that is a server command, which vmangos reads when
/// the text starts with `.` or `!` (`Chat.cpp:2031`, `:2047`) and matches by prefix
/// (`.acc pass <old> <new>`), may carry the account's. Server packets carry none.
fn secret(opcode: u16, body: &[u8]) -> bool {
    match opcode {
        opcode::CMSG_AUTH_SESSION | opcode::CMSG_JOIN_CHANNEL | opcode::CMSG_CHANNEL_PASSWORD => {
            true
        }
        opcode::CMSG_MESSAGECHAT => is_command(body),
        _ => false,
    }
}

/// Whether a `CMSG_MESSAGECHAT` body (type u32, language u32, then for a whisper or channel
/// message the target's name, then the text) has a message that starts with `.` or `!`. Both
/// layouts are tried, so a type this does not know still gets the check.
fn is_command(body: &[u8]) -> bool {
    let Some(rest) = body.get(8..) else {
        return false;
    };
    let after_target = rest
        .iter()
        .position(|&b| b == 0)
        .map(|nul| &rest[nul + 1..]);
    [Some(rest), after_target]
        .into_iter()
        .flatten()
        .any(|text| matches!(text.first(), Some(b'.' | b'!')))
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

/// The `hex=` field for a body: redacted, withheld, or the bytes. `outbound` is the client's own
/// packets, the only ones that can carry a credential; `reveal` is `WOW_MOVE_TRACE_CHAT`.
fn payload(opcode: u16, body: &[u8], outbound: bool, reveal: bool) -> String {
    if outbound && secret(opcode, body) {
        REDACTED.to_string()
    } else if private(opcode) && !reveal {
        WITHHELD.to_string()
    } else {
        hex(body)
    }
}

/// The `out` line of a client packet: `0x01dc CMSG_PING len=4 hex=…`.
fn out_line_with(opcode: u16, body: &[u8], reveal: bool) -> String {
    format!(
        "{opcode:#06x} {} len={} hex={}",
        name(opcode),
        body.len(),
        payload(opcode, body, true, reveal)
    )
}

/// The `in` line of a server packet, decoded into `events` events: `0x01dd SMSG_PONG ev=1 len=4
/// hex=…`.
fn in_line_with(opcode: u16, events: usize, body: &[u8], reveal: bool) -> String {
    format!(
        "{opcode:#06x} {} ev={events} len={} hex={}",
        name(opcode),
        body.len(),
        payload(opcode, body, false, reveal)
    )
}

pub(super) fn out_line(opcode: u16, body: &[u8]) -> String {
    out_line_with(opcode, body, reveal_private())
}

pub(super) fn in_line(opcode: u16, events: usize, body: &[u8]) -> String {
    in_line_with(opcode, events, body, reveal_private())
}

/// The `skip` line of a server packet that did not parse: its `reason` and then its whole body,
/// which the `in` line never shows.
pub(super) fn skip_line(opcode: u16, reason: &str, body: &[u8]) -> String {
    skip_line_with(opcode, reason, body, reveal_private())
}

fn skip_line_with(opcode: u16, reason: &str, body: &[u8], reveal: bool) -> String {
    let payload = payload(opcode, body, false, reveal);
    // The reader's reason ends in a ` [<n>B: <hex preview>]`, which would leak what the field hides.
    let reason = match reason.rfind(" [") {
        Some(at) if payload == WITHHELD => &reason[..at],
        _ => reason,
    };
    format!("opcode={opcode:#06x} {reason} hex={payload}")
}

/// A command the writer dropped for want of a session, as the trace's `wire` line and the log name
/// it. The `Debug` form would print a channel's password and the text of a chat message, a mail or
/// a ticket, which these lines keep out as the packet lines do.
pub(super) fn dropped(cmd: &crate::net::ClientCommand) -> String {
    use crate::net::ClientCommand as C;
    match cmd {
        C::JoinChannel { name, .. } => {
            format!("JoinChannel {{ name: {name:?}, password: {REDACTED} }}")
        }
        C::ChannelPassword { name, .. } => {
            format!("ChannelPassword {{ name: {name:?}, password: {REDACTED} }}")
        }
        C::Chat { .. } => format!("Chat {{ text: {WITHHELD} }}"),
        C::AddonMessage { .. } => format!("AddonMessage {{ text: {WITHHELD} }}"),
        C::SendMail { .. } => format!("SendMail {{ text: {WITHHELD} }}"),
        C::GmTicketCreate { .. } => format!("GmTicketCreate {{ text: {WITHHELD} }}"),
        C::GmTicketUpdate { .. } => format!("GmTicketUpdate {{ text: {WITHHELD} }}"),
        other => format!("{other:?}"),
    }
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
            out_line_with(opcode::CMSG_PING, &body, false),
            "0x01dc CMSG_PING len=4 hex=000aff7e"
        );
        assert_eq!(
            in_line_with(opcode::SMSG_PONG, 1, &body, false),
            "0x01dd SMSG_PONG ev=1 len=4 hex=000aff7e"
        );
        assert_eq!(
            in_line_with(opcode::SMSG_PONG, 0, &[], false),
            "0x01dd SMSG_PONG ev=0 len=0 hex=",
            "an empty body is an empty field"
        );
        assert_eq!(
            skip_line_with(
                opcode::SMSG_PONG,
                "short read [4B: 00 0a ff 7e]",
                &body,
                false
            ),
            "opcode=0x01dd short read [4B: 00 0a ff 7e] hex=000aff7e"
        );
    }

    /// The bytes of a login or a password never reach the file, switch or no switch.
    #[test]
    fn a_packet_that_carries_a_secret_is_redacted_always() {
        let secret = b"SECRETWORD";
        let needle = hex(secret);
        for reveal in [false, true] {
            for op in [
                opcode::CMSG_AUTH_SESSION,
                opcode::CMSG_JOIN_CHANNEL,
                opcode::CMSG_CHANNEL_PASSWORD,
            ] {
                let line = out_line_with(op, secret, reveal);
                assert!(line.ends_with("hex=<redacted>"), "{line}");
                assert!(!line.contains(&needle), "{line}");
            }
            // The length stays: it says the packet went out.
            assert!(out_line_with(opcode::CMSG_AUTH_SESSION, secret, reveal).contains("len=10"));
            // vmangos reads `.` and `!` commands and matches them by prefix.
            for command in [
                ".account password old new new",
                ".acc pass old new new",
                "!acc pass old new new",
                ".ACCOUNT create a b",
            ] {
                let line = out_line_with(opcode::CMSG_MESSAGECHAT, &chat(command), reveal);
                assert!(line.ends_with("hex=<redacted>"), "{command}: {line}");
                // The same text whispered or sent to a channel follows the target's name.
                let mut targeted = vec![7, 0, 0, 0, 0, 0, 0, 0];
                targeted.extend_from_slice(b"Alice\0");
                targeted.extend_from_slice(command.as_bytes());
                targeted.push(0);
                let line = out_line_with(opcode::CMSG_MESSAGECHAT, &targeted, reveal);
                assert!(
                    line.ends_with("hex=<redacted>"),
                    "{command} to Alice: {line}"
                );
            }
        }
        // Inbound traffic carries no credential of ours.
        assert!(in_line_with(opcode::SMSG_AUTH_RESPONSE, 1, secret, false).contains(&needle));
    }

    /// Chat body: type, language, then the text.
    fn chat(text: &str) -> Vec<u8> {
        let mut b = vec![0, 0, 0, 0, 0, 0, 0, 0];
        b.extend_from_slice(text.as_bytes());
        b.push(0);
        b
    }

    /// An inbound whisper and an outgoing say are withheld by default and shown with the switch;
    /// the length stays either way, and the lists of names go the same way.
    #[test]
    fn private_conversation_is_withheld_unless_asked_for() {
        let say = chat("meet me at the bank");
        let whisper = chat("psst, the password is hunter2 for the guild bank");
        let len = format!("len={}", say.len());

        let line = out_line_with(opcode::CMSG_MESSAGECHAT, &say, false);
        assert!(line.ends_with("hex=<withheld>"), "{line}");
        assert!(line.contains(&len), "{line}");
        assert!(!line.contains(&hex(&say)), "{line}");
        let line = out_line_with(opcode::CMSG_MESSAGECHAT, &say, true);
        assert!(line.ends_with(&format!("hex={}", hex(&say))), "{line}");

        let line = in_line_with(opcode::SMSG_MESSAGECHAT, 1, &whisper, false);
        assert!(line.ends_with("hex=<withheld>"), "{line}");
        assert!(line.contains(&format!("len={}", whisper.len())), "{line}");
        assert!(!line.contains(&hex(&whisper)), "{line}");
        let line = in_line_with(opcode::SMSG_MESSAGECHAT, 1, &whisper, true);
        assert!(line.ends_with(&format!("hex={}", hex(&whisper))), "{line}");

        for op in [
            opcode::SMSG_WHO,
            opcode::SMSG_FRIEND_LIST,
            opcode::SMSG_IGNORE_LIST,
            opcode::SMSG_GUILD_ROSTER,
            opcode::SMSG_CHANNEL_LIST,
        ] {
            let line = in_line_with(op, 1, b"Alice", false);
            assert!(line.ends_with("hex=<withheld>"), "{line}");
            assert!(in_line_with(op, 1, b"Alice", true).contains(&hex(b"Alice")));
        }
        // A single name stays traced.
        assert!(
            in_line_with(opcode::SMSG_FRIEND_STATUS, 1, b"Alice", false).contains(&hex(b"Alice"))
        );
    }

    /// An unparseable chat packet leaks nothing through the skip line's preview either.
    #[test]
    fn a_skipped_chat_packet_keeps_neither_its_bytes_nor_its_preview() {
        let body = chat("hello");
        let reason = format!(
            "opcode 0x0096 (SMSG_MESSAGECHAT): short read [{}B: 00 00 68 65]",
            body.len()
        );
        let line = skip_line_with(opcode::SMSG_MESSAGECHAT, &reason, &body, false);
        assert_eq!(
            line,
            "opcode=0x0096 opcode 0x0096 (SMSG_MESSAGECHAT): short read hex=<withheld>"
        );
        assert!(
            skip_line_with(opcode::SMSG_MESSAGECHAT, &reason, &body, true).contains(&hex(&body))
        );
    }

    /// A command the writer drops for want of a session is named without its secrets or its text.
    #[test]
    fn a_dropped_command_is_named_without_its_password_or_text() {
        use crate::net::ClientCommand as C;
        let join = dropped(&C::JoinChannel {
            name: "Trade".into(),
            password: "hunter2".into(),
        });
        assert_eq!(
            join,
            "JoinChannel { name: \"Trade\", password: <redacted> }"
        );
        let pw = dropped(&C::ChannelPassword {
            name: "Trade".into(),
            password: "hunter2".into(),
        });
        assert!(pw.contains("<redacted>") && !pw.contains("hunter2"), "{pw}");
        // Anything else keeps its `Debug` form.
        assert_eq!(dropped(&C::Logout), "Logout");
    }
}
