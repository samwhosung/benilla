//! A read-only MPQ reader for the 1.12.1 `Data/` chain: format V1/V2, every file `COMPRESS`-flagged
//! and sectored, no encrypted or single-unit files, no PTCH patches. Anything else is a hard error.
//!
//! The patch archives carry delete markers (flag `0x02000000`, size 0): the path is deleted from
//! the composite chain. [`Archive::contains`] reports one present and [`Archive::read_file`]
//! refuses it with [`Error::NotFound`], so a chain walker asks [`Archive::is_delete_marker`] and
//! stops there.
//!
//! [`Archive`] is a cheap `Arc` handle; each read opens its own file handle, so reads need no lock.
//! Every allocation sized from a header count or a sector length is capped by the file's size, so
//! a lying header fails with [`Error::Corrupt`].

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use benilla_bytes::{capped, ByteExt};

mod crypto;
use crypto::{decrypt_block, hash_string, hash_type};

/// `MPQ\x1A` as a little-endian `u32`: the archive header signature.
const SIGNATURE: u32 = 0x1A51_504D;
/// `MPQ\x1B`: a user-data header, which points past itself to the real one.
const USERDATA_SIGNATURE: u32 = 0x1B51_504D;

const FLAG_IMPLODE: u32 = 0x0000_0100;
const FLAG_COMPRESS: u32 = 0x0000_0200;
const FLAG_ENCRYPTED: u32 = 0x0001_0000;
const FLAG_SINGLE_UNIT: u32 = 0x0100_0000;
/// A patch archive's delete marker (size 0): the path is deleted from the composite chain.
const FLAG_DELETE_MARKER: u32 = 0x0200_0000;
const FLAG_EXISTS: u32 = 0x8000_0000;

/// Hash-table sentinels.
const HASH_EMPTY: u32 = 0xFFFF_FFFF; // never used: ends the probe
const HASH_DELETED: u32 = 0xFFFF_FFFE; // deleted: skip, keep probing

/// One decrypted hash-table entry (16 bytes on disk).
#[derive(Clone, Copy)]
struct HashEntry {
    name_a: u32,
    name_b: u32,
    block_index: u32,
}

/// One decrypted block-table entry (16 bytes on disk).
#[derive(Clone, Copy)]
struct BlockEntry {
    file_pos: u32,
    file_size: u32,
    flags: u32,
}

/// An archive's index, parsed once and immutable, shared behind an `Arc`.
struct Index {
    path: PathBuf,
    /// Byte offset of the MPQ header in the file: 0 in 1.12 data, but found, not assumed.
    archive_offset: u64,
    /// The uncompressed sector size, `512 << block_size`.
    sector_size: usize,
    hash_table: Vec<HashEntry>,
    block_table: Vec<BlockEntry>,
}

/// An open MPQ archive, a cheap `Clone + Send + Sync` handle.
#[derive(Clone)]
pub struct Archive {
    index: Arc<Index>,
}

/// A read error; anything outside the 1.12.1 envelope surfaces here.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    NotMpq,
    NotFound(String),
    /// A file or codec shape 1.12.1 data does not contain.
    Unsupported(String),
    Decompress(String),
    /// A count or length claims more than the archive file holds: a corrupt or truncated archive.
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

fn to_u32s(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Bytes from `pos` to EOF, saturating: the cap for every reservation sized from a header value.
fn avail_from(file_len: u64, pos: u64) -> usize {
    usize::try_from(file_len.saturating_sub(pos)).unwrap_or(usize::MAX)
}

impl Archive {
    /// Open an archive: find the header, then read and decrypt the hash and block tables.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path)?;
        let file_len = file.metadata()?.len();

        let archive_offset = find_header(&mut file, file_len)?;

        // The 32-byte V1 header: later versions only extend it, and every 1.12 table offset fits
        // a `u32`, so the V2 hi-block table is never needed.
        file.seek(SeekFrom::Start(archive_offset))?;
        let mut hdr = [0u8; 32];
        file.read_exact(&mut hdr)?;
        let signature = hdr.u32_at(0).ok_or(Error::NotMpq)?;
        debug_assert_eq!(signature, SIGNATURE);
        let block_size_shift = hdr.u16_at(14).ok_or(Error::NotMpq)?;
        // A raw header `u16`, capped before `512 << shift` can overflow; 1.12 data uses 3 (4 KiB
        // sectors), and 23 is already 4 GiB.
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

    /// Whether the archive holds `name`, case-insensitive with `/` and `\` alike. No I/O.
    pub fn contains(&self, name: &str) -> bool {
        self.index.find(name).is_some()
    }

    /// The uncompressed size of `name`, from the block table.
    pub fn file_size(&self, name: &str) -> Option<u32> {
        self.index.find(name).map(|b| b.file_size)
    }

    /// Whether `name` is a delete marker: a chain walker stops there instead of falling through.
    pub fn is_delete_marker(&self, name: &str) -> bool {
        self.index
            .find(name)
            .is_some_and(|b| b.flags & FLAG_DELETE_MARKER != 0)
    }

    /// The archive's path.
    pub fn path(&self) -> &Path {
        &self.index.path
    }

    /// Read and decompress a file by its internal path (`/` or `\`, case-insensitive).
    pub fn read_file(&self, name: &str) -> Result<Vec<u8>> {
        let idx = &self.index;
        let block = idx.find(name).ok_or_else(|| Error::NotFound(name.into()))?;

        if block.flags & FLAG_EXISTS == 0 {
            return Err(Error::NotFound(name.into()));
        }
        // A delete marker is not an empty file: the composite deleted this path.
        if block.flags & FLAG_DELETE_MARKER != 0 {
            return Err(Error::NotFound(name.into()));
        }
        // Outside the 1.12.1 envelope: refuse rather than guess.
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

        let file_len = file.metadata()?.len();
        let avail = avail_from(file_len, file_pos);

        if !compressed {
            // Stored: exactly `file_size` raw bytes, refused up front if the file cannot hold them.
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

        // Sectored: `sector_count + 1` offsets at `file_pos`, measured from it, so the patch
        // archives' SECTOR_CRC table between them and the first sector is skipped for free.
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
            // Each earlier sector yielded at most its own `want` (`decompress` is bounded), so
            // `out.len() <= file_size` and this cannot wrap.
            let want = (file_size - out.len()).min(idx.sector_size); // last sector may be short
            if comp_len == 0 {
                return Err(Error::Corrupt(format!("{name}: sector {i} is empty")));
            }

            file.seek(SeekFrom::Start(file_pos + start))?;
            let mut raw = vec![0u8; comp_len]; // proven <= avail above
            file.read_exact(&mut raw)?;

            if comp_len >= want {
                // A sector that would not shrink is stored verbatim.
                out.extend_from_slice(&raw[..want]);
            } else if implode_only {
                // IMPLODE sectors carry no method byte; 1.12.1 data has none.
                out.extend_from_slice(&decompress(0x08, &raw, want, name)?);
            } else {
                // COMPRESS: the leading byte is the codec mask, the rest the payload.
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

/// The `MPQ\x1A` header's offset: the first 512-aligned match, or where a `MPQ\x1B` user-data
/// header points.
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
                // User-data header: u32 sig, u32 user_data_size, u32 header_offset from here.
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
    // A `count` the file cannot hold is refused before the loop indexes past a short `words`.
    let avail = avail_from(file_len, pos);
    let cap = capped(count, 16, avail);
    if cap < count {
        return Err(Error::Corrupt(format!(
            "hash table: header claims {count} entries, only room for {cap} in the archive"
        )));
    }
    let mut bytes = vec![0u8; cap * 16]; // == count * 16, proven <= avail above
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
                // words[o + 2] is locale and platform, unread: 1.12.1 data is locale-neutral.
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
    // A `count` the file cannot hold is refused, as in `read_hash_table`.
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
                // words[o + 1] is compressed_size, unread: the offset table sizes the sectors.
                file_size: words[o + 2],
                flags: words[o + 3],
            }
        })
        .collect())
}

/// Decompress one sector by its codec mask; 1.12.1 data uses zlib alone.
fn decompress(method: u8, data: &[u8], expected: usize, name: &str) -> Result<Vec<u8>> {
    match method {
        0x02 => {
            use flate2::read::ZlibDecoder;
            let mut out = Vec::with_capacity(expected);
            // Bounded: a sector inflating past its slot would wrap the caller's next `want`.
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

    /// A unique temp path (pid and a counter), so parallel tests never collide.
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

    /// A minimal 32-byte V1 MPQ header, unused fields zeroed.
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

    /// `Archive::open` over `bytes` in a temp file, removed whatever the outcome.
    fn open_temp(tag: &str, bytes: &[u8]) -> Result<Archive> {
        let path = temp_path(tag);
        std::fs::write(&path, bytes).expect("write temp archive");
        let result = Archive::open(&path);
        let _ = std::fs::remove_file(&path);
        result
    }

    #[test]
    fn open_rejects_hash_table_count_bigger_than_the_file() {
        // u32::MAX hash-table entries in a 40-byte file, ~68 GiB if the reservation were uncapped.
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
        // Zero-entry tables pointing exactly at EOF: the boundary the cap must not reject.
        let hdr = header(0, 32, 32, 0, 0);
        let archive =
            open_temp("empty_tables", &hdr).expect("a well-formed empty-table archive must open");
        assert!(!archive.contains("anything"));
        assert_eq!(archive.file_size("anything"), None);
    }

    /// Like [`open_temp`], but the file stays for `read_file`, which reopens it by path.
    fn open_temp_kept(tag: &str, bytes: &[u8]) -> (Archive, PathBuf) {
        let path = temp_path(tag);
        std::fs::write(&path, bytes).expect("write temp archive");
        let archive = Archive::open(&path).expect("open temp archive");
        (archive, path)
    }

    /// A minimal V1 archive holding one stored entry, its tables encrypted with the real keys.
    fn archive_with_one_entry(name: &str, flags: u32, data: &[u8]) -> Vec<u8> {
        archive_with_one_block(name, flags, data, data.len() as u32)
    }

    /// [`archive_with_one_entry`] with `file_size` set apart from the data: a lying block table.
    fn archive_with_one_block(name: &str, flags: u32, data: &[u8], file_size: u32) -> Vec<u8> {
        use crypto::{encrypt_block, hash_type};
        const HASH_SLOTS: u32 = 4; // a power of two: the reader masks with len - 1
        let hash_pos = 32u32;
        let block_pos = hash_pos + HASH_SLOTS * 16;
        let data_pos = block_pos + 16; // one 16-byte block entry

        // Hash table (plaintext): every slot empty (0xFFFFFFFF) except our name's home slot.
        let mut hash = vec![0xFFFF_FFFFu32; HASH_SLOTS as usize * 4];
        let slot = (hash_string(name, hash_type::TABLE_OFFSET) & (HASH_SLOTS - 1)) as usize;
        hash[slot * 4] = hash_string(name, hash_type::NAME_A);
        hash[slot * 4 + 1] = hash_string(name, hash_type::NAME_B);
        hash[slot * 4 + 2] = 0; // locale | platform
        hash[slot * 4 + 3] = 0; // block index 0
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

    /// The control for the delete-marker test: the same builder's plain entry reads back.
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

    #[test]
    fn delete_marker_is_not_a_readable_empty_file() {
        let name = "Interface\\FrameXML\\ClassTrainerFrame.xml";
        let (arc, path) = open_temp_kept(
            "delete_marker",
            &archive_with_one_entry(name, FLAG_EXISTS | FLAG_DELETE_MARKER, &[]),
        );
        assert!(arc.contains(name), "the hash entry exists");
        assert!(arc.is_delete_marker(name), "flagged as a tombstone");
        // The backing file exists, so NotFound here is the delete-marker guard.
        match arc.read_file(name) {
            Err(Error::NotFound(_)) => {}
            other => panic!("expected NotFound for a tombstone, got {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn avail_from_never_underflows_when_pos_exceeds_file_len() {
        assert_eq!(avail_from(10, 100), 0);
        assert_eq!(avail_from(100, 10), 90);
        assert_eq!(avail_from(0, 0), 0);
    }

    /// Two equal consecutive offsets name a zero-length sector, with no codec byte to read.
    #[test]
    fn an_empty_sector_is_refused_not_indexed() {
        // One 8-byte file, one sector: offsets [8, 8].
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
        // 612 bytes: a full 512-byte sector (shift 0) and a 100-byte tail; sector 0 inflates to
        // 1000 bytes, twice its slot.
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

    /// `Corrupt`, not the read's `Io`, pins that the guard fires before the buffer is sized.
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
