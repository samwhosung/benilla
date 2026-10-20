//! The sends behind the console's own commands: the ones `RegisterPlayerCommands` (`0x4049e0`)
//! registers and `whois` (`0x5adcfd`). Each body is built by the command's handler and carries no
//! account test; the server answers an account without the right (vmangos `MiscHandler.cpp:1038-1098`).

/// A C string as `CDataStore::PutCString` (`0x418430`) writes it: the bytes up to the first NUL,
/// then a NUL.
fn put_c_string(out: &mut Vec<u8>, text: &str) {
    out.extend(text.bytes().take_while(|&b| b != 0));
    out.push(0);
}

fn put_floats(out: &mut Vec<u8>, values: &[f32]) {
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// `CMSG_TELEPORT_TO_UNIT` (`0x404b31`): the whole argument text of `port`, as one string.
pub fn teleport_to_unit(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 1);
    put_c_string(&mut out, text);
    out
}

/// `MSG_MOVE_TELEPORT_CHEAT` (`0x404cdc`, from `port`) and `CMSG_MOVE_SET_RAW_POSITION`
/// (`0x4051e4`, from `setrawpos`): x, y, z and the facing in radians, as four floats.
pub fn raw_position(pos: [f32; 3], facing: f32) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    put_floats(&mut out, &[pos[0], pos[1], pos[2], facing]);
    out
}

/// `CMSG_WORLD_TELEPORT` (`0x404fc8`, from `worldport`): the millisecond clock (`0x42c010`), the
/// map id, then x, y, z and the facing in radians.
pub fn world_teleport(time_ms: u32, map: u32, pos: [f32; 3], facing: f32) -> Vec<u8> {
    let mut out = Vec::with_capacity(24);
    out.extend_from_slice(&time_ms.to_le_bytes());
    out.extend_from_slice(&map.to_le_bytes());
    put_floats(&mut out, &[pos[0], pos[1], pos[2], facing]);
    out
}

/// `CMSG_WHOIS` (`0x5adf1f`): the command's whole argument text, as one string.
pub fn whois(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 1);
    put_c_string(&mut out, text);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x404b31`: the argument text and its NUL, nothing else, and not past an embedded NUL.
    #[test]
    fn the_text_sends_are_one_c_string() {
        assert_eq!(teleport_to_unit("Thrall"), b"Thrall\0");
        assert_eq!(teleport_to_unit(""), b"\0", "no argument is still a string");
        assert_eq!(whois("Thrall"), b"Thrall\0");
        assert_eq!(whois("Thr\0all"), b"Thr\0");
    }

    /// `0x404c4f`-`0x404cdc` and `0x405157`-`0x4051e4`: x, y, z, facing, little-endian floats.
    #[test]
    fn the_position_sends_are_four_floats() {
        let want: Vec<u8> = [
            0x00, 0x00, 0x80, 0x3f, // 1.0
            0x00, 0x00, 0x00, 0xc0, // -2.0
            0x00, 0x00, 0x40, 0x40, // 3.0
            0xdb, 0x0f, 0xc9, 0x3f, // pi / 2
        ]
        .to_vec();
        assert_eq!(
            raw_position([1.0, -2.0, 3.0], std::f32::consts::FRAC_PI_2),
            want
        );
    }

    /// `0x404f45`-`0x404f90`: the clock, the map, then the four floats.
    #[test]
    fn world_teleport_is_the_clock_the_map_and_the_position() {
        let body = world_teleport(
            0x0102_0304,
            1,
            [16226.0, 16257.0, 13.2],
            std::f32::consts::FRAC_PI_2,
        );
        let want: Vec<u8> = [
            0x04, 0x03, 0x02, 0x01, // the clock
            0x01, 0x00, 0x00, 0x00, // map 1
            0x00, 0x88, 0x7d, 0x46, // 16226.0
            0x00, 0x04, 0x7e, 0x46, // 16257.0
            0x33, 0x33, 0x53, 0x41, // 13.2
            0xdb, 0x0f, 0xc9, 0x3f, // pi / 2
        ]
        .to_vec();
        assert_eq!(body, want);
    }
}
