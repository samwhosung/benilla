//! Bounds-checked byte readers + IFF chunk iteration shared by the format parsers.
//! Three primitives, nothing else:
//!
//! - [`ByteExt`] — fallible little-endian accessors on `[u8]`. There is deliberately **no**
//!   panicking variant: parsers map `None` to their own truncation error with `?`, so an unguarded
//!   offset is a compile-shape impossibility, not a convention.
//! - [`chunks`] — the IFF `(magic, payload)` iterator every chunked WoW format walks. **Lenient by
//!   documented property**: iteration stops cleanly at a truncated 8-byte header, and a payload
//!   whose declared size overruns the buffer is clamped to the buffer end (real 1.12 art needs the
//!   tolerance — e.g. the trailing-junk chunks some Classic files carry).
//! - [`capped`] — the allocation guard for count-driven `Vec::with_capacity`: reserve at most what
//!   the remaining input could possibly hold, so a corrupt header count can never reserve more
//!   than the file's own size.
//!
//! Magic bytes are yielded exactly as stored (WoW writes IFF magics reversed on disk, so callers
//! match `b"DHOM"` for MOHD — same as the pre-0064 per-crate iterators).

/// Fallible little-endian reads at an offset. `None` iff the read would run past the end.
pub trait ByteExt {
    fn u8_at(&self, o: usize) -> Option<u8>;
    fn u16_at(&self, o: usize) -> Option<u16>;
    fn u32_at(&self, o: usize) -> Option<u32>;
    fn i32_at(&self, o: usize) -> Option<i32>;
    fn f32_at(&self, o: usize) -> Option<f32>;
    /// A length-checked sub-slice: `n` bytes starting at `o`.
    fn bytes_at(&self, o: usize, n: usize) -> Option<&[u8]>;
}

impl ByteExt for [u8] {
    #[inline]
    fn u8_at(&self, o: usize) -> Option<u8> {
        self.get(o).copied()
    }
    #[inline]
    fn u16_at(&self, o: usize) -> Option<u16> {
        Some(u16::from_le_bytes(
            self.get(o..o.checked_add(2)?)?.try_into().ok()?,
        ))
    }
    #[inline]
    fn u32_at(&self, o: usize) -> Option<u32> {
        Some(u32::from_le_bytes(
            self.get(o..o.checked_add(4)?)?.try_into().ok()?,
        ))
    }
    #[inline]
    fn i32_at(&self, o: usize) -> Option<i32> {
        self.u32_at(o).map(|v| v as i32)
    }
    #[inline]
    fn f32_at(&self, o: usize) -> Option<f32> {
        self.u32_at(o).map(f32::from_bits)
    }
    #[inline]
    fn bytes_at(&self, o: usize, n: usize) -> Option<&[u8]> {
        self.get(o..o.checked_add(n)?)
    }
}

/// Iterate IFF chunks as `(magic-as-stored, payload)`. See the module doc for the (deliberate)
/// leniency: truncated header → clean stop; over-declared payload → clamped to the buffer end.
pub fn chunks(b: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        if pos.checked_add(8)? > b.len() {
            return None;
        }
        let magic = [b[pos], b[pos + 1], b[pos + 2], b[pos + 3]];
        let size = b.u32_at(pos + 4)? as usize;
        let start = pos + 8;
        // `start <= b.len()` (checked above) and `size` came from a u32, so on 64-bit targets the
        // sum cannot wrap; saturate anyway so the invariant doesn't depend on pointer width.
        let end = start.saturating_add(size).min(b.len());
        pos = end;
        Some((magic, &b[start..end]))
    })
}

/// Cap a count-driven reservation by what `avail` remaining bytes could possibly hold, so a corrupt
/// header can at worst reserve the input's own size. `elem_size` of 0 is a caller bug; treat it as 1
/// rather than divide by zero. The *count* used for iteration stays the caller's — only the
/// up-front reservation is capped (a short file then fails at the bounds-checked read, not in the
/// allocator).
pub fn capped(count: usize, elem_size: usize, avail: usize) -> usize {
    count.min(avail / elem_size.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_in_bounds() {
        let b: &[u8] = &[0x01, 0x02, 0x03, 0x04, 0x05];
        assert_eq!(b.u8_at(4), Some(0x05));
        assert_eq!(b.u16_at(0), Some(0x0201));
        assert_eq!(b.u32_at(1), Some(0x05040302));
        assert_eq!(b.i32_at(0), Some(0x04030201));
        assert_eq!(b.f32_at(0), Some(f32::from_bits(0x04030201)));
        assert_eq!(b.bytes_at(2, 3), Some(&b[2..5]));
    }

    #[test]
    fn reads_out_of_bounds_are_none() {
        let b: &[u8] = &[0x01, 0x02, 0x03];
        assert_eq!(b.u8_at(3), None);
        assert_eq!(b.u16_at(2), None);
        assert_eq!(b.u32_at(0), None);
        assert_eq!(b.bytes_at(1, 3), None);
        // Offsets near usize::MAX must not wrap into a "valid" range.
        assert_eq!(b.u32_at(usize::MAX - 1), None);
        assert_eq!(b.bytes_at(usize::MAX, 8), None);
    }

    #[test]
    fn chunk_iteration_walks_well_formed_input() {
        // Two chunks: "ABCD" (2 bytes) then "EFGH" (0 bytes).
        let mut b = Vec::new();
        b.extend_from_slice(b"ABCD");
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&[0xAA, 0xBB]);
        b.extend_from_slice(b"EFGH");
        b.extend_from_slice(&0u32.to_le_bytes());
        let got: Vec<_> = chunks(&b).collect();
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].0, got[0].1), (*b"ABCD", &[0xAA, 0xBB][..]));
        assert_eq!((got[1].0, got[1].1), (*b"EFGH", &[][..]));
    }

    #[test]
    fn chunk_iteration_is_lenient() {
        // Over-declared size clamps to the buffer end.
        let mut b = Vec::new();
        b.extend_from_slice(b"ABCD");
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        b.extend_from_slice(&[0x01]);
        let got: Vec<_> = chunks(&b).collect();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, &[0x01][..]);
        // A truncated header (< 8 bytes remaining) stops iteration cleanly.
        assert_eq!(chunks(&b[..7]).count(), 0);
        assert_eq!(chunks(&[]).count(), 0);
    }

    #[test]
    fn capped_bounds_reservations() {
        assert_eq!(capped(10, 4, 1000), 10); // plausible count passes through
        assert_eq!(capped(u32::MAX as usize, 48, 96), 2); // corrupt count → what fits
        assert_eq!(capped(5, 0, 16), 5); // elem_size 0 treated as 1, no div-by-zero
        assert_eq!(capped(5, 4, 0), 0); // empty input reserves nothing
    }
}
