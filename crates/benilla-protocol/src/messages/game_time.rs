//! The server's game-time messages (`gametime/messages` in benilla-re): the packed date and the
//! length check of the four bodies that carry one or a rate. Their handlers register at
//! `ClientInitializeGameTime` (`0x6c5d70`) and each reads its fields with `IsRead` (`0x401d10`)
//! as the test of a whole body.

use std::io;

/// The 28-byte record `WowTime::WowDecodeTime` (`0x642c30`) makes of a packed date, seven `i32`.
/// Each field is a shift and a mask of the dword (`0x642b30`); a field of all ones is -1, bit 31 is
/// ignored, and no field is range-checked, so an hour reaches 30 and a minute 62. The day holds the
/// day - 1, the month the month - 1 and the year the year - 2000.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WowTime {
    /// Bits 0 to 5.
    pub minute: i32,
    /// Bits 6 to 10.
    pub hour: i32,
    /// Bits 11 to 13.
    pub weekday: i32,
    /// Bits 14 to 19.
    pub day: i32,
    /// Bits 20 to 23.
    pub month: i32,
    /// Bits 24 to 28.
    pub year: i32,
    /// Bits 29 and 30, which nothing that takes a date reads.
    pub spare: i32,
}

impl WowTime {
    /// The clock's time before any message: -1 in the six fields and 0 in the last (`0x642300`).
    pub const UNSET: Self = Self {
        minute: -1,
        hour: -1,
        weekday: -1,
        day: -1,
        month: -1,
        year: -1,
        spare: 0,
    };

    /// `0x642c30` on a packed date.
    pub fn decode(packed: u32) -> Self {
        // `0x642b3c` and its six siblings: a field equal to its mask is stored as -1.
        let field = |shift: u32, mask: u32| match (packed >> shift) & mask {
            m if m == mask => -1,
            v => v as i32,
        };
        Self {
            minute: field(0, 0x3f),
            hour: field(6, 0x1f),
            weekday: field(11, 0x7),
            day: field(14, 0x3f),
            month: field(20, 0xf),
            year: field(24, 0x1f),
            spare: field(29, 0x3),
        }
    }
}

/// The handlers' test that a body was read to its end. A body of another length is refused whole:
/// the line `Malformed message recieved: Id = %d, Len = %d, Read = %d` (`0x86c4d0`, class 0) says the
/// opcode, the store's size, which counts the opcode's two bytes, and its cursor, and nothing is
/// applied. A longer body leaves the cursor at the opcode plus the fields; a short read sets it to
/// the size plus one (`0x418ec4`, `0x418ec6`, `0x419144`, `0x419146`).
pub(super) fn exact_body(opcode: u16, body: &[u8], want: usize) -> io::Result<()> {
    let len = body.len() + 2;
    let read = match body.len().cmp(&want) {
        std::cmp::Ordering::Equal => return Ok(()),
        std::cmp::Ordering::Greater => 2 + want,
        std::cmp::Ordering::Less => len + 1,
    };
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("Malformed message recieved: Id = {opcode}, Len = {len}, Read = {read}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::super::{opcode, parse_server, ServerPacket};
    use super::*;

    /// Bytes from a hex string, so each golden reads as the packet does.
    fn hx(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// `0x642b30`: each field at 0, 1, its mask less one and its mask, alone, and among ones.
    #[test]
    fn a_packed_date_decodes_each_field_and_maps_all_ones_to_unset() {
        // (shift, mask) of minute, hour, weekday, day, month, year, spare.
        let fields = [
            (0u32, 0x3fu32),
            (6, 0x1f),
            (11, 0x7),
            (14, 0x3f),
            (20, 0xf),
            (24, 0x1f),
            (29, 0x3),
        ];
        let pick = |t: WowTime, i: usize| {
            [t.minute, t.hour, t.weekday, t.day, t.month, t.year, t.spare][i]
        };
        for (i, &(shift, mask)) in fields.iter().enumerate() {
            for v in [0, 1, mask - 1, mask] {
                let want = if v == mask { -1 } else { v as i32 };
                let alone = WowTime::decode(v << shift);
                let among = WowTime::decode(!(mask << shift) | (v << shift));
                assert_eq!(pick(alone, i), want, "field {i} = {v} alone");
                assert_eq!(pick(among, i), want, "field {i} = {v} among ones");
                for (j, _) in fields.iter().enumerate().filter(|&(j, _)| j != i) {
                    assert_eq!(pick(alone, j), 0, "field {j} beside field {i} = {v}");
                    assert_eq!(pick(among, j), -1, "field {j} among ones beside {i} = {v}");
                }
            }
        }
        // Bit 31 is ignored: alone it decodes to zero in every field.
        assert_eq!(
            WowTime::decode(0x8000_0000),
            WowTime::decode(0),
            "bit 31 is no field"
        );
        assert_eq!(
            WowTime::decode(0xffff_ffff),
            WowTime {
                spare: -1,
                ..WowTime::UNSET
            }
        );
    }

    /// 2026-10-18 (a Sunday), 14:30: minute 30, hour 14, weekday 0, day 17, month 9, year 26.
    const SUNDAY_1430: &str = "9e43941a";

    #[test]
    fn the_sunday_date_is_what_its_bytes_say() {
        assert_eq!(
            WowTime::decode(u32::from_le_bytes(hx(SUNDAY_1430).try_into().unwrap())),
            WowTime {
                minute: 30,
                hour: 14,
                weekday: 0,
                day: 17,
                month: 9,
                year: 26,
                spare: 0,
            }
        );
    }

    fn time_of(p: ServerPacket) -> WowTime {
        match p {
            ServerPacket::GameTimeUpdate { time } | ServerPacket::GameTimeSet { time } => time,
            _ => panic!("not a game-time packet"),
        }
    }

    /// `SMSG_GAMESPEED_SET` (`0x6c5de0`): one `f32`.
    #[test]
    fn the_game_speed_body_is_one_float_and_nothing_else() {
        // 2.0f32.
        match parse_server(opcode::SMSG_GAMESPEED_SET, &hx("00000040")).unwrap() {
            ServerPacket::GameSpeedSet { speed } => assert_eq!(speed, 2.0),
            _ => panic!("not a game speed"),
        }
        let refused = |body: &[u8]| {
            parse_server(opcode::SMSG_GAMESPEED_SET, body)
                .err()
                .expect("a body of another length is refused")
                .to_string()
        };
        assert_eq!(
            refused(&hx("0000004000")),
            "Malformed message recieved: Id = 71, Len = 7, Read = 6"
        );
        assert_eq!(
            refused(&hx("000040")),
            "Malformed message recieved: Id = 71, Len = 5, Read = 6"
        );
        assert_eq!(
            refused(&[]),
            "Malformed message recieved: Id = 71, Len = 2, Read = 3"
        );
    }

    /// `SMSG_GAMETIME_UPDATE` (`0x6c6010`) and `SMSG_GAMETIME_SET` (`0x6c6120`): one packed date.
    #[test]
    fn the_game_time_bodies_are_one_packed_date_and_nothing_else() {
        let want = WowTime {
            minute: 30,
            hour: 14,
            weekday: 0,
            day: 17,
            month: 9,
            year: 26,
            spare: 0,
        };
        let update = parse_server(opcode::SMSG_GAMETIME_UPDATE, &hx(SUNDAY_1430)).unwrap();
        assert!(matches!(update, ServerPacket::GameTimeUpdate { .. }));
        assert_eq!(time_of(update), want);
        let set = parse_server(opcode::SMSG_GAMETIME_SET, &hx(SUNDAY_1430)).unwrap();
        assert!(matches!(set, ServerPacket::GameTimeSet { .. }));
        assert_eq!(time_of(set), want);

        for (op, id) in [
            (opcode::SMSG_GAMETIME_UPDATE, 67),
            (opcode::SMSG_GAMETIME_SET, 69),
        ] {
            let refused = |body: &str| {
                parse_server(op, &hx(body))
                    .err()
                    .expect("a body of another length is refused")
                    .to_string()
            };
            assert_eq!(
                refused("9e43941a00"),
                format!("Malformed message recieved: Id = {id}, Len = 7, Read = 6")
            );
            assert_eq!(
                refused("9e4394"),
                format!("Malformed message recieved: Id = {id}, Len = 5, Read = 6")
            );
            assert_eq!(
                refused(""),
                format!("Malformed message recieved: Id = {id}, Len = 2, Read = 3")
            );
        }
    }

    /// `SMSG_LOGIN_SETTIMESPEED` (`0x6c5e80`): a packed date then an `f32`, whole.
    #[test]
    fn the_login_time_body_is_a_date_and_a_rate_and_nothing_else() {
        // 1/60 as the f32 the stock server sends.
        let body = format!("{SUNDAY_1430}8988883c");
        match parse_server(opcode::SMSG_LOGIN_SETTIMESPEED, &hx(&body)).unwrap() {
            ServerPacket::TimeSpeed { time, speed } => {
                assert_eq!((time.hour, time.minute), (14, 30));
                assert_eq!(speed.to_bits(), 0x3c88_8889);
            }
            _ => panic!("not a login time"),
        }
        let refused = |body: &str| {
            parse_server(opcode::SMSG_LOGIN_SETTIMESPEED, &hx(body))
                .err()
                .expect("a body of another length is refused")
                .to_string()
        };
        assert_eq!(
            refused(&format!("{body}00")),
            "Malformed message recieved: Id = 66, Len = 11, Read = 10"
        );
        assert_eq!(
            refused(&format!("{SUNDAY_1430}8988")),
            "Malformed message recieved: Id = 66, Len = 8, Read = 9"
        );
        assert_eq!(
            refused("9e43"),
            "Malformed message recieved: Id = 66, Len = 4, Read = 5"
        );
        assert_eq!(
            refused(""),
            "Malformed message recieved: Id = 66, Len = 2, Read = 3"
        );
    }

    /// An unset hour and minute (all ones) reach the clock as -1, not as 31 and 63.
    #[test]
    fn a_login_time_with_unset_fields_carries_minus_one() {
        let body = hx("ffffffff00000000");
        match parse_server(opcode::SMSG_LOGIN_SETTIMESPEED, &body).unwrap() {
            ServerPacket::TimeSpeed { time, .. } => {
                assert_eq!((time.hour, time.minute), (-1, -1));
            }
            _ => panic!("not a login time"),
        }
    }
}
