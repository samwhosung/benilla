//! The world broadcasts: server messages, zone-under-attack and defense messages. The last two
//! print, senderless, on every joined channel whose `ChatChannels.dbc` row has `DEFENSE`
//! (0x10000); a `ZONE_DEP` (0x2) channel also needs the area's parent zone to be the player's.

use std::io;

use crate::wire::{read_cstring, read_u32_le};

/// `SMSG_SERVER_MESSAGE` (`Server/Packets/Misc.cpp:341-345`, handler `0x49df80`): a
/// `ServerMessages.dbc` row id whose text is a format string for the cstring (vmangos sends 1-5,
/// `World.h:62`); a missing row prints as `"[%d]: %s"` (`0x844864`).
pub(super) fn read_server_message(r: &mut &[u8]) -> io::Result<(u32, String)> {
    let message_type = read_u32_le(r)?;
    let text = read_cstring(r)?;
    Ok((message_type, text))
}

/// `SMSG_ZONE_UNDER_ATTACK` (`Server/Packets/Misc.cpp:451-454`, handler `0x49dcc0`): the attacked
/// area's `AreaTable.dbc` id, whose own name fills the text. vmangos sends it map-wide to the
/// enemy team when a player kills a guard or PvP-enabling creature, once per area per 10 s
/// (`Creature.cpp:2889`).
pub(super) fn read_zone_under_attack(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}

/// `SMSG_DEFENSE_MESSAGE` (`Maps/Map.cpp:1868-1884`, handler `0x49de30`): `u32` zone id, `u32`
/// length counting the NUL, then the text. The reference skips exactly `length` bytes
/// (`0x419ac0`) and drops the line when that overruns the packet, so an overrun errors here.
/// Deviation: text without a NUL stops at `length`, because the reference reads on out of bounds.
pub(super) fn read_defense_message(r: &mut &[u8]) -> io::Result<(u32, String)> {
    let zone_id = read_u32_le(r)?;
    let length = read_u32_le(r)? as usize;
    if length > r.len() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "SMSG_DEFENSE_MESSAGE: length {length} overruns the {} body bytes left",
                r.len()
            ),
        ));
    }
    let (window, rest) = r.split_at(length);
    *r = rest;
    let text = window.split(|b| *b == 0).next().unwrap_or(window);
    Ok((zone_id, String::from_utf8_lossy(text).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_message_is_a_type_and_a_string() {
        let mut body = 2u32.to_le_bytes().to_vec();
        body.extend_from_slice(b"15 Minutes\0");
        assert_eq!(
            read_server_message(&mut &body[..]).unwrap(),
            (2, "15 Minutes".to_string())
        );
    }

    #[test]
    fn a_cancel_message_carries_an_empty_text() {
        let mut body = 4u32.to_le_bytes().to_vec();
        body.push(0);
        assert_eq!(
            read_server_message(&mut &body[..]).unwrap(),
            (4, String::new())
        );
    }

    #[test]
    fn zone_under_attack_is_the_bare_area_id() {
        let body = 40u32.to_le_bytes();
        assert_eq!(read_zone_under_attack(&mut &body[..]).unwrap(), 40);
    }

    #[test]
    fn a_defense_message_skips_its_length_and_reads_the_text() {
        let text = b"The Eastern Plaguelands tower has been taken!";
        let mut body = 139u32.to_le_bytes().to_vec();
        body.extend_from_slice(&(text.len() as u32 + 1).to_le_bytes());
        body.extend_from_slice(text);
        body.push(0);
        let (zone, out) = read_defense_message(&mut &body[..]).unwrap();
        assert_eq!(zone, 139);
        assert_eq!(out, String::from_utf8_lossy(text));
    }

    #[test]
    fn a_length_that_overruns_the_body_is_refused() {
        let mut body = 139u32.to_le_bytes().to_vec();
        body.extend_from_slice(&8u32.to_le_bytes());
        assert!(read_defense_message(&mut &body[..]).is_err());

        // Even with bytes present: 8 declared, 4 supplied.
        let mut body = 139u32.to_le_bytes().to_vec();
        body.extend_from_slice(&8u32.to_le_bytes());
        body.extend_from_slice(b"abc\0");
        assert!(read_defense_message(&mut &body[..]).is_err());
    }

    #[test]
    fn a_window_without_a_terminator_is_taken_whole() {
        let mut body = 139u32.to_le_bytes().to_vec();
        body.extend_from_slice(&3u32.to_le_bytes());
        body.extend_from_slice(b"abcTRAILING");
        assert_eq!(
            read_defense_message(&mut &body[..]).unwrap(),
            (139, "abc".to_string())
        );
    }
}
