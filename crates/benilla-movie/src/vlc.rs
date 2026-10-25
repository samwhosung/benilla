//! The variable-length codes of MPEG-4 Part 2 (ISO/IEC 14496-2, Annex B) that a Simple-profile
//! stream uses, written as the standard prints them and decoded through one flat lookup per table.
//!
//! The macroblock, CBPY and motion vector tables are shared with ITU-T H.263 (Tables 7, 8, 12 and
//! 14 there); the two coefficient tables are 14496-2's Tables B-16 (intra) and B-17 (inter).

use std::sync::LazyLock;

use crate::bits::BitReader;

/// A prefix code as a lookup on its longest code's length: the entry for any `bits`-bit window is
/// the value and length of the code it starts with, length 0 for a window no code starts.
pub(crate) struct Vlc {
    bits: u32,
    table: Vec<(u16, u8)>,
}

impl Vlc {
    /// Build from `(code, value)` pairs, the code as the standard prints it (spaces ignored).
    fn build(codes: &[(&str, u16)]) -> Vlc {
        let parsed: Vec<(u32, u32, u16)> = codes
            .iter()
            .map(|&(s, v)| {
                let (mut code, mut len) = (0u32, 0u32);
                for c in s.bytes().filter(|c| *c != b' ') {
                    debug_assert!(c == b'0' || c == b'1');
                    code = code << 1 | u32::from(c == b'1');
                    len += 1;
                }
                (code, len, v)
            })
            .collect();
        let bits = parsed.iter().map(|p| p.1).max().unwrap_or(1);
        let mut table = vec![(0u16, 0u8); 1 << bits];
        for &(code, len, v) in &parsed {
            let shift = bits - len;
            for tail in 0..(1u32 << shift) {
                let slot = &mut table[((code << shift) | tail) as usize];
                debug_assert_eq!(slot.1, 0, "codes overlap");
                *slot = (v, len as u8);
            }
        }
        Vlc { bits, table }
    }

    /// Decode one code, or `None` when the window starts no code.
    #[inline]
    pub(crate) fn decode(&self, r: &mut BitReader) -> Option<u16> {
        let (v, len) = self.table[r.peek(self.bits) as usize];
        if len == 0 {
            return None;
        }
        r.skip(u32::from(len));
        Some(v)
    }
}

/// MCBPC's stuffing code, and the value it decodes to.
pub(crate) const STUFFING: u16 = 0xff;

/// `mcbpc` for I-VOPs (H.263 Table 7): `mb_type << 2 | cbpc`.
pub(crate) static MCBPC_I: LazyLock<Vlc> = LazyLock::new(|| {
    Vlc::build(&[
        ("1", 3 << 2),
        ("001", 3 << 2 | 1),
        ("010", 3 << 2 | 2),
        ("011", 3 << 2 | 3),
        ("0001", 4 << 2),
        ("0000 01", 4 << 2 | 1),
        ("0000 10", 4 << 2 | 2),
        ("0000 11", 4 << 2 | 3),
        ("0000 0000 1", STUFFING),
    ])
});

/// `mcbpc` for P-VOPs (H.263 Table 8, without its type 5): `mb_type << 2 | cbpc`.
pub(crate) static MCBPC_P: LazyLock<Vlc> = LazyLock::new(|| {
    Vlc::build(&[
        ("1", 0),
        ("0011", 1),
        ("0010", 2),
        ("0001 01", 3),
        ("011", 1 << 2),
        ("0000 111", 1 << 2 | 1),
        ("0000 110", 1 << 2 | 2),
        ("0000 0010 1", 1 << 2 | 3),
        ("010", 2 << 2),
        ("0000 101", 2 << 2 | 1),
        ("0000 100", 2 << 2 | 2),
        ("0000 0101", 2 << 2 | 3),
        ("0001 1", 3 << 2),
        ("0000 0100", 3 << 2 | 1),
        ("0000 0011", 3 << 2 | 2),
        ("0000 011", 3 << 2 | 3),
        ("0001 00", 4 << 2),
        ("0000 0010 0", 4 << 2 | 1),
        ("0000 0001 1", 4 << 2 | 2),
        ("0000 0001 0", 4 << 2 | 3),
        ("0000 0000 1", STUFFING),
    ])
});

/// `cbpy` (H.263 Table 12) as the intra pattern, block 0 in bit 3; an inter macroblock's
/// pattern is its complement.
pub(crate) static CBPY: LazyLock<Vlc> = LazyLock::new(|| {
    Vlc::build(&[
        ("0011", 0),
        ("0010 1", 1),
        ("0010 0", 2),
        ("1001", 3),
        ("0001 1", 4),
        ("0111", 5),
        ("0000 10", 6),
        ("1011", 7),
        ("0001 0", 8),
        ("0000 11", 9),
        ("0101", 10),
        ("1010", 11),
        ("0100", 12),
        ("1000", 13),
        ("0110", 14),
        ("11", 15),
    ])
});

/// `dct_dc_size_luminance`.
pub(crate) static DC_SIZE_LUMA: LazyLock<Vlc> = LazyLock::new(|| {
    Vlc::build(&[
        ("011", 0),
        ("11", 1),
        ("10", 2),
        ("010", 3),
        ("001", 4),
        ("0001", 5),
        ("0000 1", 6),
        ("0000 01", 7),
        ("0000 001", 8),
        ("0000 0001", 9),
        ("0000 0000 1", 10),
        ("0000 0000 01", 11),
        ("0000 0000 001", 12),
    ])
});

/// `dct_dc_size_chrominance`.
pub(crate) static DC_SIZE_CHROMA: LazyLock<Vlc> = LazyLock::new(|| {
    Vlc::build(&[
        ("11", 0),
        ("10", 1),
        ("01", 2),
        ("001", 3),
        ("0001", 4),
        ("0000 1", 5),
        ("0000 01", 6),
        ("0000 001", 7),
        ("0000 0001", 8),
        ("0000 0000 1", 9),
        ("0000 0000 01", 10),
        ("0000 0000 001", 11),
        ("0000 0000 0001", 12),
    ])
});

/// `horizontal_mv_data` and `vertical_mv_data` (H.263 Table 14), sign included, as
/// `motion_code + 32`: the H.263 index, whose half-pel "vector difference" is half the code.
pub(crate) static MVD: LazyLock<Vlc> = LazyLock::new(|| {
    const CODES: [&str; 65] = [
        "0000 0000 0010 1",
        "0000 0000 0011 1",
        "0000 0000 0101",
        "0000 0000 0111",
        "0000 0000 1001",
        "0000 0000 1011",
        "0000 0000 1101",
        "0000 0000 1111",
        "0000 0001 001",
        "0000 0001 011",
        "0000 0001 101",
        "0000 0001 111",
        "0000 0010 001",
        "0000 0010 011",
        "0000 0010 101",
        "0000 0010 111",
        "0000 0011 001",
        "0000 0011 011",
        "0000 0011 101",
        "0000 0011 111",
        "0000 0100 001",
        "0000 0100 011",
        "0000 0100 11",
        "0000 0101 01",
        "0000 0101 11",
        "0000 0111",
        "0000 1001",
        "0000 1011",
        "0000 111",
        "0001 1",
        "0011",
        "011",
        "1",
        "010",
        "0010",
        "0001 0",
        "0000 110",
        "0000 1010",
        "0000 1000",
        "0000 0110",
        "0000 0101 10",
        "0000 0101 00",
        "0000 0100 10",
        "0000 0100 010",
        "0000 0100 000",
        "0000 0011 110",
        "0000 0011 100",
        "0000 0011 010",
        "0000 0011 000",
        "0000 0010 110",
        "0000 0010 100",
        "0000 0010 010",
        "0000 0010 000",
        "0000 0001 110",
        "0000 0001 100",
        "0000 0001 010",
        "0000 0001 000",
        "0000 0000 1110",
        "0000 0000 1100",
        "0000 0000 1010",
        "0000 0000 1000",
        "0000 0000 0110",
        "0000 0000 0100",
        "0000 0000 0011 0",
        "0000 0000 0010 0",
    ];
    let pairs: Vec<(&str, u16)> = CODES
        .iter()
        .enumerate()
        .map(|(i, c)| (*c, i as u16))
        .collect();
    Vlc::build(&pairs)
});

/// A coefficient event: `(last, run, |level|)`, its code without the sign bit that follows it.
type Event = (&'static str, u8, u8, u8);

/// Table B-16, the intra TCOEF codes, in (last, run, level) order.
#[rustfmt::skip]
const INTRA_EVENTS: [Event; 102] = [
    ("10", 0, 0, 1), ("110", 0, 0, 2), ("1111", 0, 0, 3), ("0110 1", 0, 0, 4), ("0110 0", 0, 0, 5),
    ("0101 01", 0, 0, 6), ("0100 11", 0, 0, 7), ("0100 10", 0, 0, 8), ("0010 111", 0, 0, 9),
    ("0001 1111", 0, 0, 10), ("0001 1110", 0, 0, 11), ("0001 1101", 0, 0, 12),
    ("0001 0010 1", 0, 0, 13), ("0001 0010 0", 0, 0, 14), ("0001 0001 1", 0, 0, 15),
    ("0001 0000 1", 0, 0, 16), ("0000 1000 01", 0, 0, 17), ("0000 1000 00", 0, 0, 18),
    ("0000 0011 11", 0, 0, 19), ("0000 0011 10", 0, 0, 20), ("0000 0000 111", 0, 0, 21),
    ("0000 0000 110", 0, 0, 22), ("0000 0100 000", 0, 0, 23), ("0000 0100 001", 0, 0, 24),
    ("0000 0101 0000", 0, 0, 25), ("0000 0101 0001", 0, 0, 26), ("0000 0101 0010", 0, 0, 27),
    ("1110", 0, 1, 1), ("0101 00", 0, 1, 2), ("0010 110", 0, 1, 3), ("0001 1100", 0, 1, 4),
    ("0001 0000 0", 0, 1, 5), ("0000 1111 1", 0, 1, 6), ("0000 0011 01", 0, 1, 7),
    ("0000 0100 010", 0, 1, 8), ("0000 0101 0011", 0, 1, 9), ("0000 0101 0101", 0, 1, 10),
    ("0101 1", 0, 2, 1), ("0010 101", 0, 2, 2), ("0000 1111 0", 0, 2, 3),
    ("0000 0011 00", 0, 2, 4), ("0000 0101 0110", 0, 2, 5), ("0100 01", 0, 3, 1),
    ("0001 1011", 0, 3, 2), ("0000 1110 1", 0, 3, 3), ("0000 0010 11", 0, 3, 4),
    ("0100 00", 0, 4, 1), ("0001 0001 0", 0, 4, 2), ("0000 0010 10", 0, 4, 3),
    ("0011 01", 0, 5, 1), ("0000 1110 0", 0, 5, 2), ("0000 0010 00", 0, 5, 3),
    ("0010 010", 0, 6, 1), ("0000 1101 1", 0, 6, 2), ("0000 0101 0100", 0, 6, 3),
    ("0010 100", 0, 7, 1), ("0000 1101 0", 0, 7, 2), ("0000 0101 0111", 0, 7, 3),
    ("0001 1001", 0, 8, 1), ("0000 0010 01", 0, 8, 2), ("0001 1000", 0, 9, 1),
    ("0000 0100 011", 0, 9, 2), ("0001 0111", 0, 10, 1), ("0000 1100 1", 0, 11, 1),
    ("0000 1100 0", 0, 12, 1), ("0000 0001 11", 0, 13, 1), ("0000 0101 1000", 0, 14, 1),
    ("0111", 1, 0, 1), ("0011 00", 1, 0, 2), ("0001 0110", 1, 0, 3), ("0000 1011 1", 1, 0, 4),
    ("0000 0001 10", 1, 0, 5), ("0000 0000 101", 1, 0, 6), ("0000 0000 100", 1, 0, 7),
    ("0000 0101 1001", 1, 0, 8), ("0011 11", 1, 1, 1), ("0000 1011 0", 1, 1, 2),
    ("0000 0001 01", 1, 1, 3), ("0011 10", 1, 2, 1), ("0000 0001 00", 1, 2, 2),
    ("0010 001", 1, 3, 1), ("0000 0100 100", 1, 3, 2), ("0010 000", 1, 4, 1),
    ("0000 0100 101", 1, 4, 2), ("0010 011", 1, 5, 1), ("0000 0101 1010", 1, 5, 2),
    ("0001 0101", 1, 6, 1), ("0000 0101 1011", 1, 6, 2), ("0001 0100", 1, 7, 1),
    ("0001 0011", 1, 8, 1), ("0001 1010", 1, 9, 1), ("0000 1010 1", 1, 10, 1),
    ("0000 1010 0", 1, 11, 1), ("0000 1001 1", 1, 12, 1), ("0000 1001 0", 1, 13, 1),
    ("0000 1000 1", 1, 14, 1), ("0000 0100 110", 1, 15, 1), ("0000 0100 111", 1, 16, 1),
    ("0000 0101 1100", 1, 17, 1), ("0000 0101 1101", 1, 18, 1), ("0000 0101 1110", 1, 19, 1),
    ("0000 0101 1111", 1, 20, 1),
];

/// Table B-17, the inter TCOEF codes (H.263 Table 16), in (last, run, level) order.
#[rustfmt::skip]
const INTER_EVENTS: [Event; 102] = [
    ("10", 0, 0, 1), ("1111", 0, 0, 2), ("0101 01", 0, 0, 3), ("0010 111", 0, 0, 4),
    ("0001 1111", 0, 0, 5), ("0001 0010 1", 0, 0, 6), ("0001 0010 0", 0, 0, 7),
    ("0000 1000 01", 0, 0, 8), ("0000 1000 00", 0, 0, 9), ("0000 0000 111", 0, 0, 10),
    ("0000 0000 110", 0, 0, 11), ("0000 0100 000", 0, 0, 12), ("110", 0, 1, 1),
    ("0101 00", 0, 1, 2), ("0001 1110", 0, 1, 3), ("0000 0011 11", 0, 1, 4),
    ("0000 0100 001", 0, 1, 5), ("0000 0101 0000", 0, 1, 6), ("1110", 0, 2, 1),
    ("0001 1101", 0, 2, 2), ("0000 0011 10", 0, 2, 3), ("0000 0101 0001", 0, 2, 4),
    ("0110 1", 0, 3, 1), ("0001 0001 1", 0, 3, 2), ("0000 0011 01", 0, 3, 3), ("0110 0", 0, 4, 1),
    ("0001 0001 0", 0, 4, 2), ("0000 0101 0010", 0, 4, 3), ("0101 1", 0, 5, 1),
    ("0000 0011 00", 0, 5, 2), ("0000 0101 0011", 0, 5, 3), ("0100 11", 0, 6, 1),
    ("0000 0010 11", 0, 6, 2), ("0000 0101 0100", 0, 6, 3), ("0100 10", 0, 7, 1),
    ("0000 0010 10", 0, 7, 2), ("0100 01", 0, 8, 1), ("0000 0010 01", 0, 8, 2),
    ("0100 00", 0, 9, 1), ("0000 0010 00", 0, 9, 2), ("0010 110", 0, 10, 1),
    ("0000 0101 0101", 0, 10, 2), ("0010 101", 0, 11, 1), ("0010 100", 0, 12, 1),
    ("0001 1100", 0, 13, 1), ("0001 1011", 0, 14, 1), ("0001 0000 1", 0, 15, 1),
    ("0001 0000 0", 0, 16, 1), ("0000 1111 1", 0, 17, 1), ("0000 1111 0", 0, 18, 1),
    ("0000 1110 1", 0, 19, 1), ("0000 1110 0", 0, 20, 1), ("0000 1101 1", 0, 21, 1),
    ("0000 1101 0", 0, 22, 1), ("0000 0100 010", 0, 23, 1), ("0000 0100 011", 0, 24, 1),
    ("0000 0101 0110", 0, 25, 1), ("0000 0101 0111", 0, 26, 1), ("0111", 1, 0, 1),
    ("0000 1100 1", 1, 0, 2), ("0000 0000 101", 1, 0, 3), ("0011 11", 1, 1, 1),
    ("0000 0000 100", 1, 1, 2), ("0011 10", 1, 2, 1), ("0011 01", 1, 3, 1), ("0011 00", 1, 4, 1),
    ("0010 011", 1, 5, 1), ("0010 010", 1, 6, 1), ("0010 001", 1, 7, 1), ("0010 000", 1, 8, 1),
    ("0001 1010", 1, 9, 1), ("0001 1001", 1, 10, 1), ("0001 1000", 1, 11, 1),
    ("0001 0111", 1, 12, 1), ("0001 0110", 1, 13, 1), ("0001 0101", 1, 14, 1),
    ("0001 0100", 1, 15, 1), ("0001 0011", 1, 16, 1), ("0000 1100 0", 1, 17, 1),
    ("0000 1011 1", 1, 18, 1), ("0000 1011 0", 1, 19, 1), ("0000 1010 1", 1, 20, 1),
    ("0000 1010 0", 1, 21, 1), ("0000 1001 1", 1, 22, 1), ("0000 1001 0", 1, 23, 1),
    ("0000 1000 1", 1, 24, 1), ("0000 0001 11", 1, 25, 1), ("0000 0001 10", 1, 26, 1),
    ("0000 0001 01", 1, 27, 1), ("0000 0001 00", 1, 28, 1), ("0000 0100 100", 1, 29, 1),
    ("0000 0100 101", 1, 30, 1), ("0000 0100 110", 1, 31, 1), ("0000 0100 111", 1, 32, 1),
    ("0000 0101 1000", 1, 33, 1), ("0000 0101 1001", 1, 34, 1), ("0000 0101 1010", 1, 35, 1),
    ("0000 0101 1011", 1, 36, 1), ("0000 0101 1100", 1, 37, 1), ("0000 0101 1101", 1, 38, 1),
    ("0000 0101 1110", 1, 39, 1), ("0000 0101 1111", 1, 40, 1),
];

/// The TCOEF escape code, which no sign bit follows.
const ESCAPE_CODE: &str = "0000 011";
const ESCAPE: u16 = 0xffff;

/// A decoded coefficient event, the level signed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Coef {
    pub(crate) last: bool,
    pub(crate) run: u32,
    pub(crate) level: i32,
}

/// One TCOEF table with its escape limits, LMAX and RMAX, which
/// are the largest level per (last, run) and the largest run per (last, level) the table codes.
pub(crate) struct Tcoef {
    vlc: Vlc,
    lmax: [[u8; 64]; 2],
    rmax: [[u8; 64]; 2],
}

fn pack(last: u8, run: u8, level: u8) -> u16 {
    u16::from(last) << 11 | u16::from(run) << 5 | u16::from(level)
}

impl Tcoef {
    fn build(events: &[Event]) -> Tcoef {
        let mut pairs: Vec<(&str, u16)> = events
            .iter()
            .map(|&(c, last, run, level)| (c, pack(last, run, level)))
            .collect();
        pairs.push((ESCAPE_CODE, ESCAPE));
        let mut lmax = [[0u8; 64]; 2];
        let mut rmax = [[0u8; 64]; 2];
        for &(_, last, run, level) in events {
            let l = &mut lmax[last as usize][run as usize];
            *l = (*l).max(level);
            let r = &mut rmax[last as usize][level as usize];
            *r = (*r).max(run);
        }
        Tcoef {
            vlc: Vlc::build(&pairs),
            lmax,
            rmax,
        }
    }

    /// One table code and its sign bit: `Ok(None)` for the escape, `Err` for no code.
    #[inline]
    fn event(&self, r: &mut BitReader) -> Result<Option<Coef>, &'static str> {
        let v = self.vlc.decode(r).ok_or("invalid TCOEF code")?;
        if v == ESCAPE {
            return Ok(None);
        }
        let level = i32::from(v & 31);
        Ok(Some(Coef {
            last: v & (1 << 11) != 0,
            run: u32::from(v >> 5 & 63),
            level: if r.bit() { -level } else { level },
        }))
    }

    /// One coefficient event, escapes resolved (14496-2, "Escape code"): type 1 (`ESC 0`) adds
    /// LMAX to the level, type 2 (`ESC 10`) adds RMAX + 1 to the run, type 3 (`ESC 11`) is fixed
    /// length: last, a 6-bit run, a marker, a 12-bit two's-complement level and a marker.
    #[inline]
    pub(crate) fn decode(&self, r: &mut BitReader) -> Result<Coef, &'static str> {
        if let Some(c) = self.event(r)? {
            return Ok(c);
        }
        if !r.bit() {
            let mut c = self.event(r)?.ok_or("TCOEF escape inside an escape")?;
            let add = i32::from(self.lmax[c.last as usize][c.run as usize]);
            c.level += if c.level < 0 { -add } else { add };
            return Ok(c);
        }
        if !r.bit() {
            let mut c = self.event(r)?.ok_or("TCOEF escape inside an escape")?;
            c.run += u32::from(self.rmax[c.last as usize][c.level.unsigned_abs() as usize]) + 1;
            return Ok(c);
        }
        let last = r.bit();
        let run = r.read(6);
        if !r.bit() {
            return Err("missing marker in a TCOEF escape");
        }
        let level = ((r.read(12) << 20) as i32) >> 20;
        if !r.bit() {
            return Err("missing marker in a TCOEF escape");
        }
        if level == 0 || level == -2048 {
            return Err("forbidden TCOEF escape level");
        }
        Ok(Coef { last, run, level })
    }
}

pub(crate) static TCOEF_INTRA: LazyLock<Tcoef> = LazyLock::new(|| Tcoef::build(&INTRA_EVENTS));
pub(crate) static TCOEF_INTER: LazyLock<Tcoef> = LazyLock::new(|| Tcoef::build(&INTER_EVENTS));

#[cfg(test)]
mod tests {
    use super::*;

    /// Write `(code, len)` strings into bytes, MSB first.
    fn bits(parts: &[&str]) -> Vec<u8> {
        let s: String = parts.concat().chars().filter(|c| *c != ' ').collect();
        let mut out = vec![0u8; s.len().div_ceil(8) + 8];
        for (i, c) in s.bytes().enumerate() {
            if c == b'1' {
                out[i / 8] |= 0x80 >> (i % 8);
            }
        }
        out
    }

    /// The Kraft sum of a table's codes, in units of 2^-`unit`, and whether no code prefixes
    /// another.
    fn kraft(codes: &[String], unit: u32) -> (u64, bool) {
        let sum = codes.iter().map(|c| 1u64 << (unit - c.len() as u32)).sum();
        let prefix_free = codes.iter().enumerate().all(|(i, a)| {
            codes
                .iter()
                .enumerate()
                .all(|(j, b)| i == j || !b.starts_with(a))
        });
        (sum, prefix_free)
    }

    fn strip(s: &str) -> String {
        s.chars().filter(|c| *c != ' ').collect()
    }

    #[test]
    fn coefficient_tables_are_prefix_codes_with_one_free_9_bit_prefix() {
        // Each table's 102 codes, a sign bit after each, and the escape fill all but the
        // `0000 0000 0` prefix, which keeps a start code from being emulated.
        for events in [&INTRA_EVENTS, &INTER_EVENTS] {
            // A code and its two signs weigh what the code alone does.
            let mut codes: Vec<String> = events.iter().map(|e| strip(e.0)).collect();
            codes.push(strip(ESCAPE_CODE));
            let (sum, prefix_free) = kraft(&codes, 13);
            assert!(prefix_free);
            assert_eq!(sum, (1 << 13) - (1 << 4));
        }
        // Table B-16 and B-17 share their code words and differ only in what each one means.
        let mut a: Vec<String> = INTRA_EVENTS.iter().map(|e| strip(e.0)).collect();
        let mut b: Vec<String> = INTER_EVENTS.iter().map(|e| strip(e.0)).collect();
        a.sort();
        b.sort();
        assert_eq!(a, b);
    }

    #[test]
    fn escape_limits_match_the_standards_lmax_and_rmax_tables() {
        let intra = &*TCOEF_INTRA;
        let inter = &*TCOEF_INTER;
        // The standard's LMAX tables, intra then inter, by run.
        let lmax_intra_0 = [27, 10, 5, 4, 3, 3, 3, 3, 2, 2, 1, 1, 1, 1, 1];
        let lmax_intra_1 = [
            8, 3, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        ];
        let lmax_inter_0 = [12, 6, 4, 3, 3, 3, 3, 2, 2, 2, 2];
        assert_eq!(&intra.lmax[0][..15], &lmax_intra_0);
        assert_eq!(&intra.lmax[1][..21], &lmax_intra_1);
        assert_eq!(&inter.lmax[0][..11], &lmax_inter_0);
        assert!(inter.lmax[0][11..27].iter().all(|&l| l == 1));
        assert_eq!(&inter.lmax[1][..2], &[3, 2]);
        assert!(inter.lmax[1][2..41].iter().all(|&l| l == 1));
        // The standard's RMAX tables, intra then inter, by level.
        assert_eq!(&intra.rmax[0][1..6], &[14, 9, 7, 3, 2]);
        assert!(intra.rmax[0][6..11].iter().all(|&r| r == 1));
        assert!(intra.rmax[0][11..28].iter().all(|&r| r == 0));
        assert_eq!(&intra.rmax[1][1..4], &[20, 6, 1]);
        assert!(intra.rmax[1][4..9].iter().all(|&r| r == 0));
        assert_eq!(&inter.rmax[0][1..7], &[26, 10, 6, 2, 1, 1]);
        assert!(inter.rmax[0][7..13].iter().all(|&r| r == 0));
        assert_eq!(&inter.rmax[1][1..4], &[40, 1, 0]);
    }

    #[test]
    fn decodes_table_codes_and_all_three_escapes() {
        let data = bits(&[
            "0101 01",
            "1", // intra (0, 0, 6), negative
            "0000 0101 1111",
            "0", // intra (1, 20, 1)
            "0000 011",
            "0",
            "110",
            "1", // type 1: (0, 0, 2) + LMAX 27 = -29
            "0000 011",
            "10",
            "0111",
            "0", // type 2: (1, 0, 1), run + RMAX 20 + 1 = 21
            "0000 011",
            "11",
            "1",
            "000101",
            "1",
            "1111 1111 1101",
            "1", // type 3: (1, 5, -3)
        ]);
        let mut r = BitReader::new(&data);
        let t = &*TCOEF_INTRA;
        let c = |last, run, level| Coef { last, run, level };
        assert_eq!(t.decode(&mut r), Ok(c(false, 0, -6)));
        assert_eq!(t.decode(&mut r), Ok(c(true, 20, 1)));
        assert_eq!(t.decode(&mut r), Ok(c(false, 0, -29)));
        assert_eq!(t.decode(&mut r), Ok(c(true, 21, 1)));
        assert_eq!(t.decode(&mut r), Ok(c(true, 5, -3)));
        // The inter table reads the same word as another event: `0101 01` is (0, 0, 3) there.
        let data = bits(&["0101 01", "0"]);
        assert_eq!(
            TCOEF_INTER.decode(&mut BitReader::new(&data)),
            Ok(c(false, 0, 3))
        );
        // The all-zero word starts no code.
        assert!(TCOEF_INTER
            .decode(&mut BitReader::new(&[0, 0, 0, 0]))
            .is_err());
    }

    #[test]
    fn macroblock_and_vector_tables_decode_every_code_they_list() {
        // Every MVD code decodes to its own index, sign included, and the table is complete but for
        // its all-zero corner.
        let codes: Vec<String> = (0..65u16)
            .map(|i| {
                let v = &*MVD;
                let mut found = None;
                for w in 0..(1u32 << v.bits) {
                    let (val, len) = v.table[w as usize];
                    if val == i && len > 0 {
                        let s = format!(
                            "{:0width$b}",
                            w >> (v.bits - u32::from(len)),
                            width = len as usize
                        );
                        found = Some(s);
                        break;
                    }
                }
                found.expect("every motion code has a word")
            })
            .collect();
        let (sum, prefix_free) = kraft(&codes, 13);
        assert!(prefix_free);
        assert_eq!(sum, (1 << 13) - 4);
        for (i, c) in codes.iter().enumerate() {
            let data = bits(&[c]);
            assert_eq!(MVD.decode(&mut BitReader::new(&data)), Some(i as u16));
        }
        // H.263's index 32 is the zero vector, 33 is +0.5 pel (motion code +1), 31 is -1.
        assert_eq!(codes[32], "1");
        assert_eq!(codes[33], "010");
        assert_eq!(codes[31], "011");

        let data = bits(&["0000 0000 1", "0001 01", "11"]);
        let mut r = BitReader::new(&data);
        assert_eq!(MCBPC_P.decode(&mut r), Some(STUFFING));
        assert_eq!(MCBPC_P.decode(&mut r), Some(3));
        assert_eq!(CBPY.decode(&mut r), Some(15));
        let data = bits(&["0000 0000 001", "0000 0000 0001"]);
        let mut r = BitReader::new(&data);
        assert_eq!(DC_SIZE_LUMA.decode(&mut r), Some(12));
        assert_eq!(DC_SIZE_CHROMA.decode(&mut r), Some(12));
    }
}
