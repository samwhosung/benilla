//! An MSB-first bit reader. Reads past the end yield zero bits and set [`BitReader::overrun`], so a
//! decoder checks once per macroblock instead of on every read: the all-zero string is no valid
//! code in any MPEG-4 table, so a truncated stream fails at the next code it reads.

pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    /// The bit position from the start of `data`.
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// The next 64 bits from the byte holding `pos`, zero-filled past the end.
    #[inline]
    fn window(&self) -> u64 {
        let byte = self.pos >> 3;
        match self.data.get(byte..byte + 8) {
            Some(b) => u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
            None => {
                let mut w = [0u8; 8];
                if let Some(tail) = self.data.get(byte..) {
                    w[..tail.len()].copy_from_slice(tail);
                }
                u64::from_be_bytes(w)
            }
        }
    }

    /// The next `n` bits (at most 32) without consuming them.
    #[inline]
    pub(crate) fn peek(&self, n: u32) -> u32 {
        debug_assert!(n <= 32);
        if n == 0 {
            return 0;
        }
        ((self.window() << (self.pos & 7)) >> (64 - n)) as u32
    }

    #[inline]
    pub(crate) fn skip(&mut self, n: u32) {
        self.pos += n as usize;
    }

    #[inline]
    pub(crate) fn read(&mut self, n: u32) -> u32 {
        let v = self.peek(n);
        self.skip(n);
        v
    }

    #[inline]
    pub(crate) fn bit(&mut self) -> bool {
        self.read(1) != 0
    }

    /// Whether a read went past the end of the data.
    pub(crate) fn overrun(&self) -> bool {
        self.pos > self.data.len() * 8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_msb_first_across_bytes_and_zero_fills_past_the_end() {
        let mut r = BitReader::new(&[0b1010_1100, 0b0101_0011]);
        assert_eq!(r.read(3), 0b101);
        assert_eq!(r.peek(7), 0b011_0001);
        assert_eq!(r.read(9), 0b0_1100_0101);
        assert!(!r.overrun());
        assert_eq!(r.read(8), 0b0011_0000);
        assert!(r.overrun());
    }

    #[test]
    fn reads_a_full_word_at_any_alignment() {
        let data = [0xde, 0xad, 0xbe, 0xef, 0x12, 0x34, 0x56, 0x78, 0x9a];
        for shift in 0..8 {
            let mut r = BitReader::new(&data);
            r.skip(shift);
            let all = u128::from_be_bytes({
                let mut b = [0u8; 16];
                b[..9].copy_from_slice(&data);
                b
            });
            let want = ((all << shift) >> 96) as u32;
            assert_eq!(r.read(32), want, "shift {shift}");
        }
    }
}
