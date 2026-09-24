//! A read-only MPQ archive reader for **WoW 1.12.1 (build 5875)** — in-repo, replacing `wow-mpq`.
//!
//! Deliberately narrow to what the real `Data/` chain actually is (verified by probing every archive):
//! format **V1/V2**, every file `COMPRESS`-flagged and **sectored**, **no encrypted files**, **no
//! single-unit**, **no PTCH** incremental patches. So this does header parse → hash/block **table**
//! decrypt → name lookup → sectored read (per-sector inflate). It does *not* write archives, decrypt
//! file data, verify signatures, or load the `(attributes)` table — the wasteful per-open parse that
//! motivated 0021. Anything outside the verified envelope is a hard error, never a silent guess.
//!
//! **Delete-markers.** The patch archives *do* carry file tombstones — a block entry flagged
//! `MPQ_FILE_DELETE_MARKER` (`0x02000000`), `size == 0`, meaning "this path is deleted from the
//! composite; do not use any lower-priority archive's copy." (`patch.MPQ` uses these to remove stock
//! FrameXML it replaces with backported addons — decision 0246.) These are **not** readable files:
//! [`Archive::read_file`] refuses one with [`Error::NotFound`] rather than returning the empty buffer
//! a naive `size == 0` stored-read would (the silent mislead that cost 0246 a day), and
//! [`Archive::is_delete_marker`] lets the chain walker see a tombstone and stop instead of falling
//! through. A delete-marked entry still *exists* in the hash table, so [`Archive::contains`] reports
//! it present — the chain must consult [`Archive::is_delete_marker`] to tell "deleted" from "readable".
//!
//! ## Concurrency
//! The parsed [`Index`] (header offsets + decrypted hash/block tables) is immutable, so [`Archive`] is
//! a cheap `Arc` handle that is `Send + Sync` and `Clone`. [`Archive::read_file`] is `&self`: it opens
//! a **fresh** OS file handle for the read, so concurrent reads share no seek state and need no lock.
//! Opening the handle is a cheap syscall; the expensive table parse happens once, in [`Archive::open`].
//!
//! Correctness was proven by a differential pass against `wow-mpq` over real archives — 37,971 reads
//! byte-identical across all 13 (the throwaway oracle test is in git history). The
//! `benilla-formats` loaders now exercise this reader end-to-end against real data on every run.
//!
//! Byte access goes through `benilla-bytes`, migrated minimally: the local rd_u16/
//! rd_u32 are gone in favor of `ByteExt`, and every allocation sized from a header/block-table count
//! or a sector length — all attacker-controllable in a hostile archive — is capped by what the
//! archive file could actually hold (`capped`), so a lying header fails with [`Error::Corrupt`]
//! instead of aborting the allocator. Nothing else here changes.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use benilla_bytes::{capped, ByteExt};

mod crypto;
use crypto::{decrypt_block, hash_string, hash_type};

/// `MPQ\x1A` — the archive header signature (little-endian u32).
const SIGNATURE: u32 = 0x1A51_504D;
/// `MPQ\x1B` — the user-data header signature (some archives offset the real header past it).
const USERDATA_SIGNATURE: u32 = 0x1B51_504D;

// Block-table entry flags (only the ones the 1.12.1 envelope can present; others are asserted absent).
const FLAG_IMPLODE: u32 = 0x0000_0100;
const FLAG_COMPRESS: u32 = 0x0000_0200;
const FLAG_ENCRYPTED: u32 = 0x0001_0000;
const FLAG_SINGLE_UNIT: u32 = 0x0100_0000;
/// A patch-archive tombstone: the path is deleted from the composite chain (size 0). Recognised so a
/// tombstone reads as [`Error::NotFound`], not an empty buffer.
const FLAG_DELETE_MARKER: u32 = 0x0200_0000;
const FLAG_EXISTS: u32 = 0x8000_0000;

/// Hash-table sentinels.
const HASH_EMPTY: u32 = 0xFFFF_FFFF; // slot never used — stop probing
const HASH_DELETED: u32 = 0xFFFF_FFFE; // slot deleted — skip, keep probing

/// One decrypted hash-table entry (16 bytes on disk).
#[derive(Clone, Copy)]
struct HashEntry {
    name_a: u32,
    name_b: u32,
    block_index: u32,
}

/// One decrypted block-table entry (16 bytes on disk). The on-disk `compressed_size` (word 1) is
/// skipped — sector sizes come from the offset table, so it's never needed for reading.
#[derive(Clone, Copy)]
struct BlockEntry {
    file_pos: u32,
    file_size: u32,
    flags: u32,
}

/// The immutable, parsed-once index of an archive: where the data starts, the sector size, and the
/// decrypted hash/block tables. Shared behind an `Arc` so [`Archive`] clones are cheap and reads borrow
/// it without locking.
struct Index {
    path: PathBuf,
    /// Byte offset of the MPQ header within the file (0 for vanilla, but found, not assumed).
    archive_offset: u64,
    /// `512 << block_size` — the uncompressed sector size.
    sector_size: usize,
    hash_table: Vec<HashEntry>,
    block_table: Vec<BlockEntry>,
}

/// An open MPQ archive: a cheap, `Clone`, `Send + Sync` handle over a shared [`Index`]. Reads open a
/// fresh OS handle, so they are `&self` and lock-free.
#[derive(Clone)]
pub struct Archive {
    index: Arc<Index>,
}

/// Errors a read can produce. Anything outside the verified 1.12.1 envelope (encrypted file, unknown
/// codec, …) surfaces here rather than being papered over.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    NotMpq,
    NotFound(String),
    /// A file or codec shape the 1.12.1 envelope shouldn't contain — investigate, don't guess.
    Unsupported(String),
    Decompress(String),
    /// A header/block-table count or sector length claims more than the archive file could possibly
    /// hold. Caught by the capped reservations before the allocator would; a corrupt
    /// or truncated archive, not a coding bug.
    Corrupt(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::NotMpq => write!(f, "not an MPQ archive (no header signature found)"),
            Error::NotFound(n) => write!(f, "file not in archive: {n}"),
            Error::Unsupported(m) => write!(f, "unsupported (outside 1.12.1 envelope): {m}"),
            Error::Decompress(m) => write!(f, "decompress: {m}"),
            Error::Corrupt(m) => write!(f, "corrupt archive: {m}"),
        }
    }
}

impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

type Result<T> = std::result::Result<T, Error>;

/// Reinterpret a byte buffer (length a multiple of 4) as a `Vec<u32>` (little-endian) for table decrypt.
fn to_u32s(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Bytes actually available in the archive file from `pos` to EOF, clamped into `usize`. Table counts
/// and sector lengths are attacker-controllable header/block-table values; every
/// reservation sized from one of them is capped against this, so a lying value reserves at most the
/// archive's own size and the mismatch surfaces as a clean [`Error::Corrupt`], never an allocator abort.
fn avail_from(file_len: u64, pos: u64) -> usize {
    usize::try_from(file_len.saturating_sub(pos)).unwrap_or(usize::MAX)
}

impl Archive {
    /// Open and index an archive: find the header, then read+decrypt the hash and block tables. The
    /// file handle used here is dropped on return; reads reopen their own.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path)?;
        let file_len = file.metadata()?.len();

        // Find the header. Vanilla archives put it at 0, but the format allows a user-data header
        // first, so scan 512-aligned boundaries for the `MPQ\x1A` signature (resolving `MPQ\x1B`).
        let archive_offset = find_header(&mut file, file_len)?;

        // Read the 32-byte V1 header prefix (V2/V3/V4 only extend it; the V1 fields are all we need —
        // every real table offset fits in u32, so the V2 hi-block table is irrelevant here).
        file.seek(SeekFrom::Start(archive_offset))?;
        let mut hdr = [0u8; 32];
        file.read_exact(&mut hdr)?;
        // Every field below is a fixed offset within a buffer that was just `read_exact`'d to exactly
        // 32 bytes, so `u16_at`/`u32_at` can only return `None` if that invariant is somehow broken —
        // map it to `Error::NotMpq` (a header we can't decode isn't a valid one) rather than trust it.
        let signature = hdr.u32_at(0).ok_or(Error::NotMpq)?;
        debug_assert_eq!(signature, SIGNATURE);
        let block_size_shift = hdr.u16_at(14).ok_or(Error::NotMpq)?;
        // The shift is a raw header u16: unchecked, a corrupt value ≥ 55 overflows the `usize`
        // shift (a panic in debug, a masked shift in release — both wrong). 23 (512 B << 23 = 4 GiB
        // sectors) is already far beyond any real archive (1.12 data uses 3 → 4 KiB sectors).
        if block_size_shift > 23 {
            return Err(Error::Corrupt(format!(
                "block size shift {block_size_shift} exceeds the sanity cap"
            )));
        }
        let sector_size = 512usize << block_size_shift;
        let hash_table_pos = hdr.u32_at(16).ok_or(Error::NotMpq)? as u64;
        let block_table_pos = hdr.u32_at(20).ok_or(Error::NotMpq)? as u64;
        let hash_table_size = hdr.u32_at(24).ok_or(Error::NotMpq)? as usize;
        let block_table_size = hdr.u32_at(28).ok_or(Error::NotMpq)? as usize;

        let hash_table = read_hash_table(
            &mut file,
            archive_offset + hash_table_pos,
            hash_table_size,
            file_len,
        )?;
        let block_table = read_block_table(
            &mut file,
            archive_offset + block_table_pos,
            block_table_size,
            file_len,
        )?;

        Ok(Archive {
            index: Arc::new(Index {
                path,
                archive_offset,
                sector_size,
                hash_table,
                block_table,
            }),
        })
    }

    /// Whether the archive holds `name` (case-insensitive; `/` and `\` equivalent). No I/O.
    pub fn contains(&self, name: &str) -> bool {
        self.index.find(name).is_some()
    }

    /// The uncompressed size of `name`, if present. No I/O (read from the block table).
    pub fn file_size(&self, name: &str) -> Option<u32> {
        self.index.find(name).map(|b| b.file_size)
    }

    /// Whether `name`'s entry is a **delete-marker** (a patch tombstone) rather than a readable file.
    /// [`Archive::contains`] is still `true` for one (the hash entry exists); the chain walker calls
    /// this to tell "the client deleted this path" (stop, don't fall through) from "readable file".
    /// No I/O.
    pub fn is_delete_marker(&self, name: &str) -> bool {
        self.index
            .find(name)
            .is_some_and(|b| b.flags & FLAG_DELETE_MARKER != 0)
    }

    /// The archive's path.
    pub fn path(&self) -> &Path {
        &self.index.path
    }

    /// Read and decompress a file by its internal path. `/` or `\`; case-insensitive. `&self`: opens a
    /// fresh OS handle, so concurrent reads need no lock.
    pub fn read_file(&self, name: &str) -> Result<Vec<u8>> {
        let idx = &self.index;
        let block = idx.find(name).ok_or_else(|| Error::NotFound(name.into()))?;

        if block.flags & FLAG_EXISTS == 0 {
            return Err(Error::NotFound(name.into()));
        }
        // A delete-marker is a tombstone, not a file: `size == 0` with no data. Reading it as a
        // stored 0-byte file would hand back an empty buffer that looks like a real (empty) file —
        // the silent mislead of 0246. It is "not found" (the composite deleted this path).
        if block.flags & FLAG_DELETE_MARKER != 0 {
            return Err(Error::NotFound(name.into()));
        }
        // The verified 1.12.1 envelope: no encrypted files, no single-unit. Refuse rather than guess.
        if block.flags & FLAG_ENCRYPTED != 0 {
            return Err(Error::Unsupported(format!("encrypted file {name}")));
        }
        if block.flags & FLAG_SINGLE_UNIT != 0 {
            return Err(Error::Unsupported(format!("single-unit file {name}")));
        }

        let mut file = File::open(&idx.path)?;
        let file_pos = idx.archive_offset + block.file_pos as u64;
        let file_size = block.file_size as usize;
        let compressed = block.flags & (FLAG_COMPRESS | FLAG_IMPLODE) != 0;
        let implode_only = block.flags & FLAG_IMPLODE != 0 && block.flags & FLAG_COMPRESS == 0;

        // `file_size` (hence `sector_count`) and each sector's `comp_len` all come from the
        // block-table entry / on-disk offset table — the same attacker-controllable-header shape as
        // the hash/block tables. Cap every reservation below by what the archive file
        // could actually hold from `file_pos` on, so a corrupt entry fails cleanly instead of the
        // allocator aborting.
        let file_len = file.metadata()?.len();
        let avail = avail_from(file_len, file_pos);

        if !compressed {
            // Stored: raw bytes, exactly file_size — which the archive must hold from `file_pos`
            // on. A stored entry claiming more is a corrupt block table, refused before the
            // buffer is sized rather than after a 4 GiB one comes back short.
            if capped(file_size, 1, avail) < file_size {
                return Err(Error::Corrupt(format!(
                    "{name}: stored size ({file_size}) larger than the archive"
                )));
            }
            file.seek(SeekFrom::Start(file_pos))?;
            let mut out = vec![0u8; file_size]; // proven <= avail above
            file.read_exact(&mut out)?;
            return Ok(out);
        }

        // Sectored read. The offset table is `sector_count + 1` u32s at file_pos; sector offsets are
        // absolute from file_pos, so a SECTOR_CRC table (present only on the patch archives) sits
        // between the offset table and the first sector and is skipped for free.
        let sector_count = file_size.div_ceil(idx.sector_size);
        let otab_len = sector_count.checked_add(1).ok_or_else(|| {
            Error::Corrupt(format!("{name}: sector count overflow ({sector_count})"))
        })?;
        let otab_cap = capped(otab_len, 4, avail);
        if otab_cap < otab_len {
            return Err(Error::Corrupt(format!(
                "{name}: sector offset table ({otab_len} entries) larger than the archive"
            )));
        }
        file.seek(SeekFrom::Start(file_pos))?;
        let mut otab = vec![0u8; otab_cap * 4]; // == otab_len * 4, proven <= avail above
        file.read_exact(&mut otab)?;
        let offsets: Vec<u32> = otab
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| {
                c.u32_at(0)
                    .ok_or_else(|| Error::Corrupt(format!("{name}: corrupt sector offset table")))
            })
            .collect::<Result<Vec<u32>>>()?;

        let mut out = Vec::with_capacity(capped(file_size, 1, avail));
        for i in 0..sector_count {
            let start = offsets[i] as u64;
            let end = offsets[i + 1] as u64;
            let comp_len = end
                .checked_sub(start)
                .ok_or_else(|| Error::Decompress(format!("{name}: sector {i} offsets reversed")))?
                as usize;
            let comp_cap = capped(comp_len, 1, avail);
            if comp_cap < comp_len {
                return Err(Error::Corrupt(format!(
                    "{name}: sector {i} length ({comp_len}) larger than the archive"
                )));
            }
            // Every earlier sector yielded at most its own `want` (`decompress` is bounded to
            // `expected`), so `out.len() <= file_size` and this cannot wrap; `want > 0` in-loop.
            let want = (file_size - out.len()).min(idx.sector_size); // last sector may be short
                                                                     // Two equal consecutive offsets are a mis-built table, not a sector: zero bytes can
                                                                     // carry no codec byte and no payload, so refuse before anything indexes into them.
            if comp_len == 0 {
                return Err(Error::Corrupt(format!("{name}: sector {i} is empty")));
            }

            file.seek(SeekFrom::Start(file_pos + start))?;
            let mut raw = vec![0u8; comp_len]; // proven <= avail above
            file.read_exact(&mut raw)?;

            if comp_len >= want {
                // Not actually compressed (a sector that wouldn't shrink is stored verbatim).
                out.extend_from_slice(&raw[..want]);
            } else if implode_only {
                // IMPLODE files carry no per-sector method byte. (Verified absent in 1.12.1; kept for
                // completeness so the shape is explicit rather than a silent fall-through.)
                out.extend_from_slice(&decompress(0x08, &raw, want, name)?);
            } else {
                // COMPRESS: leading byte is the codec mask, rest is the payload.
                let method = raw[0]; // non-empty: refused above
                out.extend_from_slice(&decompress(method, &raw[1..], want, name)?);
            }
        }
        Ok(out)
    }
}

impl Index {
    /// Locate a file's block entry via the hash table (Blizzard's open-addressed probe).
    fn find(&self, name: &str) -> Option<BlockEntry> {
        let len = self.hash_table.len();
        if len == 0 {
            return None;
        }
        let mask = (len - 1) as u32;
        let start = (hash_string(name, hash_type::TABLE_OFFSET) & mask) as usize;
        let want_a = hash_string(name, hash_type::NAME_A);
        let want_b = hash_string(name, hash_type::NAME_B);
        for i in 0..len {
            let e = &self.hash_table[(start + i) % len];
            if e.block_index == HASH_EMPTY {
                return None; // never-used slot ends the chain
            }
            if e.block_index == HASH_DELETED {
                continue;
            }
            if e.name_a == want_a && e.name_b == want_b {
                return self.block_table.get(e.block_index as usize).copied();
            }
        }
        None
    }
}

/// Scan 512-aligned boundaries for the MPQ header, resolving a `MPQ\x1B` user-data header to the real
/// one it points at. Returns the byte offset of the `MPQ\x1A` header.
fn find_header(file: &mut File, file_len: u64) -> Result<u64> {
    let mut off = 0u64;
    while off + 4 <= file_len {
        file.seek(SeekFrom::Start(off))?;
        let mut sig = [0u8; 4];
        if file.read_exact(&mut sig).is_err() {
            break;
        }
        match u32::from_le_bytes(sig) {
            SIGNATURE => return Ok(off),
            USERDATA_SIGNATURE => {
                // user-data header: u32 sig, u32 user_data_size, u32 header_offset (relative to here).
                let mut rest = [0u8; 8];
                file.read_exact(&mut rest)?;
                let header_offset = u32::from_le_bytes([rest[4], rest[5], rest[6], rest[7]]) as u64;
                return Ok(off + header_offset);
            }
            _ => {}
        }
        off += 512;
    }
    Err(Error::NotMpq)
}

fn read_hash_table(
    file: &mut File,
    pos: u64,
    count: usize,
    file_len: u64,
) -> Result<Vec<HashEntry>> {
    // `count` is a header u32: cap the reservation by what the archive could actually
    // hold from `pos` on, so a lying header can at worst reserve the file's own size. If the capped
    // reservation had to shrink below `count`, the header is lying (or the file is truncated) — fail
    // cleanly here rather than let the entry-building loop below index past a short `words`.
    let avail = avail_from(file_len, pos);
    let cap = capped(count, 16, avail);
    if cap < count {
        return Err(Error::Corrupt(format!(
            "hash table: header claims {count} entries, only room for {cap} in the archive"
        )));
    }
    let mut bytes = vec![0u8; cap * 16]; // == count * 16, proven <= avail above — never a wild reserve
    file.seek(SeekFrom::Start(pos))?;
    file.read_exact(&mut bytes)?;
    let mut words = to_u32s(&bytes);
    decrypt_block(&mut words, hash_string("(hash table)", hash_type::FILE_KEY));
    Ok((0..count)
        .map(|i| {
            let o = i * 4;
            HashEntry {
                name_a: words[o],
                name_b: words[o + 1],
                // words[o+2] packs locale(u16)+platform(u16); 1.12.1 data is locale-neutral, ignore.
                block_index: words[o + 3],
            }
        })
        .collect())
}

fn read_block_table(
    file: &mut File,
    pos: u64,
    count: usize,
    file_len: u64,
) -> Result<Vec<BlockEntry>> {
    // Same treatment as `read_hash_table` above: `count` is an attacker-controllable header u32.
    let avail = avail_from(file_len, pos);
    let cap = capped(count, 16, avail);
    if cap < count {
        return Err(Error::Corrupt(format!(
            "block table: header claims {count} entries, only room for {cap} in the archive"
        )));
    }
    let mut bytes = vec![0u8; cap * 16]; // == count * 16, proven <= avail above
    file.seek(SeekFrom::Start(pos))?;
    file.read_exact(&mut bytes)?;
    let mut words = to_u32s(&bytes);
    decrypt_block(
        &mut words,
        hash_string("(block table)", hash_type::FILE_KEY),
    );
    Ok((0..count)
        .map(|i| {
            let o = i * 4;
            BlockEntry {
                file_pos: words[o],
                // words[o + 1] = compressed_size on disk; unused (offset table sizes sectors).
                file_size: words[o + 2],
                flags: words[o + 3],
            }
        })
        .collect())
}

/// Decompress one sector payload by its MPQ codec mask. 1.12.1 uses ZLIB (and store, handled by the
/// caller); any other mask is an error so the differential pass flags it for us to add deliberately.
fn decompress(method: u8, data: &[u8], expected: usize, name: &str) -> Result<Vec<u8>> {
    match method {
        0x02 => {
            use flate2::read::ZlibDecoder;
            let mut out = Vec::with_capacity(expected);
            // Bounded to `expected`: a stream that inflates past its sector's slot would push the
            // caller's output past `file_size` and wrap the next sector's `want` — a malformed
            // sector, not a shape 1.12 data has, and never the caller's arithmetic to absorb.
            ZlibDecoder::new(data)
                .take(expected as u64)
                .read_to_end(&mut out)
                .map_err(|e| Error::Decompress(format!("{name}: zlib: {e}")))?;
            Ok(out)
        }
        other => Err(Error::Unsupported(format!(
            "{name}: sector codec 0x{other:02X} not yet implemented"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A unique path under the OS temp dir — no `tempfile` dependency (decision 0064 keeps the leaf
    /// crates dependency-light), so uniqueness is hand-rolled: pid + a per-process counter, enough to
    /// never collide across parallel `cargo test` threads in this crate.
    fn temp_path(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "benilla_mpq_test_{}_{}_{}.mpq",
            std::process::id(),
            n,
            tag
        ))
    }

    /// Build a minimal 32-byte V1 MPQ header (unused fields zeroed) at the start of a buffer.
    fn header(
        block_size_shift: u16,
        hash_table_pos: u32,
        block_table_pos: u32,
        hash_table_size: u32,
        block_table_size: u32,
    ) -> [u8; 32] {
        let mut h = [0u8; 32];
        h[0..4].copy_from_slice(&SIGNATURE.to_le_bytes());
        h[14..16].copy_from_slice(&block_size_shift.to_le_bytes());
        h[16..20].copy_from_slice(&hash_table_pos.to_le_bytes());
        h[20..24].copy_from_slice(&block_table_pos.to_le_bytes());
        h[24..28].copy_from_slice(&hash_table_size.to_le_bytes());
        h[28..32].copy_from_slice(&block_table_size.to_le_bytes());
        h
    }

    /// Write `bytes` to a fresh temp file, run `Archive::open` on it, and remove the file afterwards
    /// regardless of the outcome.
    fn open_temp(tag: &str, bytes: &[u8]) -> Result<Archive> {
        let path = temp_path(tag);
        std::fs::write(&path, bytes).expect("write temp archive");
        let result = Archive::open(&path);
        let _ = std::fs::remove_file(&path);
        result
    }

    #[test]
    fn open_rejects_hash_table_count_bigger_than_the_file() {
        // A header claiming u32::MAX hash-table entries in a 40-byte file: pre-0064 this would have
        // tried `vec![0u8; u32::MAX as usize * 16]` (~68 GiB) and aborted the allocator. It must now
        // return a clean `Error::Corrupt` instead.
        let hdr = header(0, 32, 32, u32::MAX, 0);
        let mut bytes = hdr.to_vec();
        bytes.extend_from_slice(&[0u8; 8]); // pad past the header so `pos` (32) is in-bounds
        match open_temp("hash_overflow", &bytes) {
            Err(Error::Corrupt(_)) => {}
            Ok(_) => panic!("expected Error::Corrupt, got Ok(_)"),
            Err(other) => panic!("expected Error::Corrupt, got {other:?}"),
        }
    }

    #[test]
    fn open_rejects_block_table_count_bigger_than_the_file() {
        let hdr = header(0, 32, 32, 0, u32::MAX);
        let mut bytes = hdr.to_vec();
        bytes.extend_from_slice(&[0u8; 8]);
        match open_temp("block_overflow", &bytes) {
            Err(Error::Corrupt(_)) => {}
            Ok(_) => panic!("expected Error::Corrupt, got Ok(_)"),
            Err(other) => panic!("expected Error::Corrupt, got {other:?}"),
        }
    }

    #[test]
    fn open_accepts_a_valid_archive_with_empty_tables() {
        // The boundary case the cap must not reject: zero-entry tables pointing exactly at EOF. Proves
        // the new cap logic doesn't disturb a legitimately-shaped (if degenerate) archive.
        let hdr = header(0, 32, 32, 0, 0);
        let archive =
            open_temp("empty_tables", &hdr).expect("a well-formed empty-table archive must open");
        assert!(!archive.contains("anything"));
        assert_eq!(archive.file_size("anything"), None);
    }

    /// Like [`open_temp`] but leaves the backing file on disk so [`Archive::read_file`] (which reopens
    /// the archive by path) works; returns the path for the caller to remove.
    fn open_temp_kept(tag: &str, bytes: &[u8]) -> (Archive, PathBuf) {
        let path = temp_path(tag);
        std::fs::write(&path, bytes).expect("write temp archive");
        let archive = Archive::open(&path).expect("open temp archive");
        (archive, path)
    }

    /// Build a minimal V1 archive holding exactly one entry (`name`) with the given block `flags` and
    /// stored `data`. Tables are encrypted with the real keys so the reader's decrypt round-trips
    /// them — enough to exercise flag handling (delete-marker vs. a plain stored file).
    fn archive_with_one_entry(name: &str, flags: u32, data: &[u8]) -> Vec<u8> {
        archive_with_one_block(name, flags, data, data.len() as u32)
    }

    /// [`archive_with_one_entry`] with the block entry's `file_size` set independently of the
    /// bytes actually appended — the shape of a lying block table.
    fn archive_with_one_block(name: &str, flags: u32, data: &[u8], file_size: u32) -> Vec<u8> {
        use crypto::{encrypt_block, hash_type};
        const HASH_SLOTS: u32 = 4; // power of two — the reader masks with len-1
        let hash_pos = 32u32;
        let block_pos = hash_pos + HASH_SLOTS * 16;
        let data_pos = block_pos + 16; // one 16-byte block entry

        // Hash table (plaintext): every slot empty (0xFFFFFFFF) except our name's home slot.
        let mut hash = vec![0xFFFF_FFFFu32; HASH_SLOTS as usize * 4];
        let slot = (hash_string(name, hash_type::TABLE_OFFSET) & (HASH_SLOTS - 1)) as usize;
        hash[slot * 4] = hash_string(name, hash_type::NAME_A);
        hash[slot * 4 + 1] = hash_string(name, hash_type::NAME_B);
        hash[slot * 4 + 2] = 0; // locale | platform
        hash[slot * 4 + 3] = 0; // → block index 0
        encrypt_block(&mut hash, hash_string("(hash table)", hash_type::FILE_KEY));

        // Block table (plaintext): one entry pointing at the trailing data.
        let mut block = vec![data_pos, data.len() as u32, file_size, flags];
        encrypt_block(
            &mut block,
            hash_string("(block table)", hash_type::FILE_KEY),
        );

        let mut bytes = header(0, hash_pos, block_pos, HASH_SLOTS, 1).to_vec();
        for w in hash.into_iter().chain(block) {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        bytes.extend_from_slice(data);
        bytes
    }

    /// The builder is honest: a plain stored (EXISTS-only) entry reads its bytes back and is not a
    /// delete-marker — so the negative result in the next test can't be a broken-archive artifact.
    #[test]
    fn stored_entry_reads_its_bytes() {
        let (arc, path) = open_temp_kept(
            "stored",
            &archive_with_one_entry("Interface\\a.txt", FLAG_EXISTS, b"hello"),
        );
        assert!(arc.contains("Interface\\a.txt"));
        assert!(!arc.is_delete_marker("Interface\\a.txt"));
        assert_eq!(arc.read_file("Interface\\a.txt").unwrap(), b"hello");
        let _ = std::fs::remove_file(&path);
    }

    /// A delete-marker (`EXISTS | DELETE_MARKER`, size 0) is a tombstone, not an empty file: it is
    /// still `contains`ed (the hash entry exists) and flagged `is_delete_marker`, but `read_file`
    /// refuses it with `NotFound` rather than handing back an empty buffer (the silent
    /// mislead that made the trainer arc quote a stale, deleted stock file).
    #[test]
    fn delete_marker_is_not_a_readable_empty_file() {
        let name = "Interface\\FrameXML\\ClassTrainerFrame.xml";
        let (arc, path) = open_temp_kept(
            "delete_marker",
            &archive_with_one_entry(name, FLAG_EXISTS | FLAG_DELETE_MARKER, &[]),
        );
        assert!(arc.contains(name), "the hash entry exists");
        assert!(arc.is_delete_marker(name), "flagged as a tombstone");
        // The file is present on disk, so a NotFound here is the tombstone guard firing — not a
        // missing backing file.
        match arc.read_file(name) {
            Err(Error::NotFound(_)) => {}
            other => panic!("expected NotFound for a tombstone, got {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn avail_from_never_underflows_when_pos_exceeds_file_len() {
        // `pos` derived from a corrupt header can point past EOF; the helper must saturate to 0
        // rather than wrap `file_len - pos` around to a huge usize.
        assert_eq!(avail_from(10, 100), 0);
        assert_eq!(avail_from(100, 10), 90);
        assert_eq!(avail_from(0, 0), 0);
    }

    /// A sector offset table with two equal consecutive offsets names a zero-length sector; the
    /// codec-byte read (`raw[0]`) indexed that empty buffer and panicked on the first read of the
    /// file. A private server's mis-built `patch-*.MPQ` is the realistic source.
    #[test]
    fn an_empty_sector_is_refused_not_indexed() {
        // One 8-byte file, one sector: offsets [8, 8] — zero bytes for a sector that must yield 8.
        let mut data = Vec::new();
        data.extend_from_slice(&8u32.to_le_bytes());
        data.extend_from_slice(&8u32.to_le_bytes());
        let (arc, path) = open_temp_kept(
            "empty_sector",
            &archive_with_one_entry("a.bin", FLAG_EXISTS | FLAG_COMPRESS, &data),
        );
        match arc.read_file("a.bin") {
            Err(Error::Corrupt(_)) => {}
            other => panic!("expected Error::Corrupt for an empty sector, got {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    /// A sector that inflates past its own slot used to run the output past `file_size`, and the
    /// next sector's `want = file_size - out.len()` wrapped (a panic in dev, a `usize::MAX`
    /// reservation in release). The inflate is bounded to the sector's expected size instead.
    #[test]
    fn an_over_inflating_sector_cannot_run_the_output_past_file_size() {
        use flate2::{write::ZlibEncoder, Compression};
        use std::io::Write;
        // A COMPRESS sector as stored: the 0x02 (zlib) codec byte, then the stream.
        let zlib = |raw: &[u8]| {
            let mut e = ZlibEncoder::new(vec![0x02u8], Compression::default());
            e.write_all(raw).unwrap();
            e.finish().unwrap()
        };
        // 612 bytes of file = one full 512-byte sector (shift 0) + a 100-byte tail. Sector 0's
        // stream inflates to 1000 bytes, twice its slot.
        let s0 = zlib(&[0u8; 1000]);
        let s1 = zlib(&[7u8; 100]);
        let otab_len = 12u32;
        let mut data = Vec::new();
        data.extend_from_slice(&otab_len.to_le_bytes());
        data.extend_from_slice(&(otab_len + s0.len() as u32).to_le_bytes());
        data.extend_from_slice(&(otab_len + (s0.len() + s1.len()) as u32).to_le_bytes());
        data.extend_from_slice(&s0);
        data.extend_from_slice(&s1);
        let (arc, path) = open_temp_kept(
            "over_inflate",
            &archive_with_one_block("a.bin", FLAG_EXISTS | FLAG_COMPRESS, &data, 612),
        );
        let out = arc
            .read_file("a.bin")
            .expect("a bounded inflate reads cleanly");
        assert_eq!(out.len(), 612);
        assert!(out[..512].iter().all(|&b| b == 0));
        assert!(out[512..].iter().all(|&b| b == 7));
        let _ = std::fs::remove_file(&path);
    }

    /// A stored (uncompressed) entry's buffer was sized straight from the block table's
    /// `file_size` — `vec![0u8; 4 GiB]` before the read that would have come back short. It is
    /// now refused against what the archive holds from the entry's position, like the sectored
    /// path's reservations; `Corrupt`, not the read's `Io`, pins that the guard fired first.
    #[test]
    fn a_stored_size_larger_than_the_archive_is_refused_before_allocating() {
        let (arc, path) = open_temp_kept(
            "stored_overflow",
            &archive_with_one_block("a.txt", FLAG_EXISTS, b"hello", u32::MAX),
        );
        match arc.read_file("a.txt") {
            Err(Error::Corrupt(_)) => {}
            other => panic!("expected Error::Corrupt for a lying stored size, got {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }
}
