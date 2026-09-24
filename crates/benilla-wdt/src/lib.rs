//! The 1.12.1 WDT (map tile table) reader, and the tile and world coordinate helpers.
//!
//! A `.wdt` holds `MVER`, `MPHD`, `MAIN` and, on a WMO-only map, `MWMO` and one `MODF`. `MAIN` is a
//! 64×64 table of 8-byte entries whose flag bit 0 marks a tile with an `.adt`. `MPHD` dword 0 bit 0
//! marks a map with no terrain, whose whole world is the one building the `MODF` places: 20 of the
//! 43 shipped maps, every WMO dungeon, each with an empty `MAIN`.

use std::io::{Read, Seek, SeekFrom};

/// The client generation; 1.12.1 is the only one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WowVersion {
    Classic,
}

/// Re-export, so `benilla_wdt::version::WowVersion` resolves.
pub mod version {
    pub use super::WowVersion;
}

/// Tiles per map edge.
const MAP_SIZE: usize = 64;
const TILE_YARDS: f32 = 533.333_3;

/// World `(x, y)` → ADT tile `(tile_x, tile_y)`, clamped to `0..=63`. The axes swap: `tile_x`
/// comes from world y, `tile_y` from world x.
pub fn world_to_tile(world_x: f32, world_y: f32) -> (u32, u32) {
    let offset = 32.0 * TILE_YARDS;
    let tile_x = ((offset - world_y) / TILE_YARDS) as u32;
    let tile_y = ((offset - world_x) / TILE_YARDS) as u32;
    (tile_x.min(63), tile_y.min(63))
}

const CHUNKS_PER_TILE: u32 = 16;
const CHUNK_YARDS: f32 = TILE_YARDS / CHUNKS_PER_TILE as f32;

/// World `(x, y)` → chunk `(chunk_x, chunk_y)` on the 1024×1024 grid, clamped to `0..=1023`, with
/// [`world_to_tile`]'s axis swap and `chunk >> 4` its tile. It is the reference's streaming
/// coordinate: `0x672730` computes `fistp((17066.666 - pos) * 0.03 - 0.5)` per axis and sizes its
/// residency windows in chunks.
pub fn world_to_chunk(world_x: f32, world_y: f32) -> (u32, u32) {
    let offset = 32.0 * TILE_YARDS;
    let max = MAP_SIZE as u32 * CHUNKS_PER_TILE - 1;
    let chunk_x = ((offset - world_y) / CHUNK_YARDS) as u32;
    let chunk_y = ((offset - world_x) / CHUNK_YARDS) as u32;
    (chunk_x.min(max), chunk_y.min(max))
}

/// ADT tile → the world `(x, y)` of its max corner, the inverse of [`world_to_tile`].
pub fn tile_to_world(tile_x: u32, tile_y: u32) -> (f32, f32) {
    let offset = 32.0 * TILE_YARDS;
    (
        offset - tile_y as f32 * TILE_YARDS,
        offset - tile_x as f32 * TILE_YARDS,
    )
}

/// One map tile, as the streamer reads it.
#[derive(Debug, Clone, Copy)]
pub struct TileInfo {
    pub x: usize,
    pub y: usize,
    /// Whether this tile has an `.adt` (MAIN flag bit 0).
    pub has_adt: bool,
}

/// The one building that is a WMO-only map: the `MWMO` path and its `MODF` entry, already in
/// `benilla_formats::WmoInstance`'s convention.
#[derive(Debug, Clone)]
pub struct GlobalWmo {
    /// `MWMO` root path (e.g. `world\wmo\dungeon\az_stormwindprisons\stormwindjail.wmo`).
    pub model: String,
    /// Position in world coords (X north, Y west, Z up), taken raw: the `32·533⅓ - v` remap an
    /// ADT's placements get would put every shipped WMO-only map, which stores `(0, 0, 0)`, 24 km
    /// from its entrance.
    pub position: [f32; 3],
    /// Euler rotation in degrees (X, Y, Z), Y the heading; Dire Maul's `ry` is 180°, the rest 0.
    pub rotation: [f32; 3],
    /// MODF doodad-set index: the prop set shown beside the always-on set 0.
    pub doodad_set: u16,
    /// MODF name-set index, the placement's `WMOAreaTable.NameSetID` variant.
    pub name_set: u16,
}

/// A parsed WDT: the 64×64 tile-existence grid, plus the global WMO on a WMO-only map.
#[derive(Debug)]
pub struct WdtFile {
    /// `has_adt` per tile, indexed `y * 64 + x` (the on-disk MAIN order).
    has_adt: Vec<bool>,
    global_wmo: Option<GlobalWmo>,
}

impl WdtFile {
    pub fn get_tile(&self, x: usize, y: usize) -> Option<TileInfo> {
        if x >= MAP_SIZE || y >= MAP_SIZE {
            return None;
        }
        Some(TileInfo {
            x,
            y,
            has_adt: self.has_adt[y * MAP_SIZE + x],
        })
    }

    /// The map's one building, on a WMO-only map (`MPHD` bit 0).
    pub fn global_wmo(&self) -> Option<&GlobalWmo> {
        self.global_wmo.as_ref()
    }
}

/// Reads a [`WdtFile`] from a chunked WDT stream.
pub struct WdtReader<R> {
    reader: R,
    _version: WowVersion,
}

impl<R: Read + Seek> WdtReader<R> {
    pub fn new(reader: R, version: WowVersion) -> Self {
        Self {
            reader,
            _version: version,
        }
    }

    /// One chunk's payload, refused before the buffer is sized if the stream cannot hold it.
    fn read_payload(&mut self, size: u64, stream_end: u64) -> std::io::Result<Vec<u8>> {
        let remaining = stream_end.saturating_sub(self.reader.stream_position()?);
        if size > remaining {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "WDT chunk runs past the end of the stream",
            ));
        }
        let mut buf = vec![0u8; size as usize];
        self.reader.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Parse the WDT, walking chunks to EOF and skipping all but `MPHD`, `MAIN`, `MWMO` and `MODF`.
    pub fn read(&mut self) -> std::io::Result<WdtFile> {
        let start = self.reader.stream_position()?;
        let stream_end = self.reader.seek(SeekFrom::End(0))?;
        self.reader.seek(SeekFrom::Start(start))?;
        let mut has_adt: Option<Vec<bool>> = None;
        let mut wmo_only = false;
        let mut wmo_path: Option<String> = None;
        let mut modf: Option<Vec<u8>> = None;
        loop {
            let mut hdr = [0u8; 8];
            if !read_full(&mut self.reader, &mut hdr)? {
                break; // clean EOF
            }
            // Chunk magics are stored reversed on disk: MAIN is "NIAM".
            let size = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as u64;
            match &hdr[0..4] {
                // MPHD: the reference reads only dword 0 bit 0, the no-terrain flag (`0x694810`).
                b"DHPM" if size >= 4 => {
                    let buf = self.read_payload(size, stream_end)?;
                    wmo_only = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) & 0x1 != 0;
                }
                b"NIAM" => {
                    // 64×64 entries × 8 bytes (flags u32, area_id u32); bit 0 of flags = has_adt.
                    let count = MAP_SIZE * MAP_SIZE;
                    let mut buf = vec![0u8; count * 8];
                    self.reader.read_exact(&mut buf)?;
                    let grid = (0..count)
                        .map(|i| {
                            let flags = u32::from_le_bytes([
                                buf[i * 8],
                                buf[i * 8 + 1],
                                buf[i * 8 + 2],
                                buf[i * 8 + 3],
                            ]);
                            flags & 0x1 != 0
                        })
                        .collect();
                    has_adt = Some(grid);
                }
                // MWMO: a NUL-terminated path on a WMO-only map, a zero-length stub on an ADT map,
                // so the `wmo_only` flag, not this chunk, is the gate.
                b"OMWM" if size > 0 => {
                    let buf = self.read_payload(size, stream_end)?;
                    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                    let path = String::from_utf8_lossy(&buf[..end]).into_owned();
                    if !path.is_empty() {
                        wmo_path = Some(path);
                    }
                }
                // MODF: exactly one 64-byte placement here.
                b"FDOM" if size >= 64 => {
                    modf = Some(self.read_payload(size, stream_end)?);
                }
                _ => {
                    self.reader.seek(SeekFrom::Current(size as i64))?;
                }
            }
        }
        let has_adt = has_adt.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "WDT missing MAIN chunk")
        })?;
        // The flag and a placement, or no global WMO: a stray header bit must not hide the grid.
        let global_wmo = match (wmo_only, wmo_path, modf) {
            (true, Some(model), Some(e)) => Some(GlobalWmo {
                model,
                position: [f32_at(&e, 8), f32_at(&e, 12), f32_at(&e, 16)],
                rotation: [f32_at(&e, 20), f32_at(&e, 24), f32_at(&e, 28)],
                // 0x20..0x38 is the bounding box. `uniqueId` at +4 is unread: the reference
                // overwrites it from its own counter (`0xc9a320`).
                doodad_set: u16_at(&e, 58),
                name_set: u16_at(&e, 60),
            }),
            _ => None,
        };
        Ok(WdtFile {
            has_adt,
            global_wmo,
        })
    }
}

fn f32_at(b: &[u8], off: usize) -> f32 {
    b.get(off..off + 4)
        .and_then(|s| s.try_into().ok())
        .map_or(0.0, f32::from_le_bytes)
}

fn u16_at(b: &[u8], off: usize) -> u16 {
    b.get(off..off + 2)
        .and_then(|s| s.try_into().ok())
        .map_or(0, u16::from_le_bytes)
}

/// Read exactly `buf.len()` bytes; `Ok(false)` on a clean EOF before any byte.
fn read_full<R: Read>(reader: &mut R, buf: &mut [u8]) -> std::io::Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => {
                return if filled == 0 {
                    Ok(false)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "truncated WDT chunk header",
                    ))
                }
            }
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // The grid constants restated, so the tests do not read the code's own.
    const T: f32 = 533.333_3;
    const OFFSET: f32 = 32.0 * T;

    /// A minimal WDT: `MVER`, a zero `MPHD`, a `MAIN` flagging exactly `set_tiles`, and an unknown
    /// chunk after it if `trailing`.
    fn synth_wdt(set_tiles: &[(usize, usize)], trailing: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let chunk = |magic: &[u8; 4], payload: &[u8], out: &mut Vec<u8>| {
            out.extend_from_slice(magic);
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(payload);
        };
        chunk(b"REVM", &18u32.to_le_bytes(), &mut out);
        chunk(b"DHPM", &[0u8; 32], &mut out);
        let count = MAP_SIZE * MAP_SIZE;
        let mut main = vec![0u8; count * 8];
        for &(x, y) in set_tiles {
            let flags_off = (y * MAP_SIZE + x) * 8;
            main[flags_off] = 0x1; // low byte of the u32 flags carries bit 0
        }
        chunk(b"NIAM", &main, &mut out);
        if trailing {
            chunk(b"FOOB", &[0xAB; 16], &mut out);
        }
        out
    }

    fn parse(bytes: Vec<u8>) -> std::io::Result<WdtFile> {
        WdtReader::new(Cursor::new(bytes), WowVersion::Classic).read()
    }

    /// A WMO-only map: `MPHD` bit 0, an empty `MAIN`, `MWMO` and one 64-byte `MODF` of distinct
    /// values, so a column slip fails.
    fn synth_wmo_only_wdt(mphd_bit0: bool, with_modf: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let chunk = |magic: &[u8; 4], payload: &[u8], out: &mut Vec<u8>| {
            out.extend_from_slice(magic);
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(payload);
        };
        chunk(b"REVM", &18u32.to_le_bytes(), &mut out);
        let mut mphd = [0u8; 32];
        mphd[0] = u8::from(mphd_bit0);
        chunk(b"DHPM", &mphd, &mut out);
        chunk(b"NIAM", &vec![0u8; MAP_SIZE * MAP_SIZE * 8], &mut out); // no tiles at all
        chunk(b"OMWM", b"world\\wmo\\dungeon\\test\\t.wmo\0", &mut out);
        if with_modf {
            let mut e = [0u8; 64];
            e[0..4].copy_from_slice(&0u32.to_le_bytes()); // nameId
            e[4..8].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // uniqueId, unread
            for (i, v) in [11.0f32, 22.0, 33.0].iter().enumerate() {
                e[8 + i * 4..12 + i * 4].copy_from_slice(&v.to_le_bytes()); // position
            }
            for (i, v) in [1.0f32, 180.0, 2.0].iter().enumerate() {
                e[20 + i * 4..24 + i * 4].copy_from_slice(&v.to_le_bytes()); // rotation
            }
            // 0x20..0x38, the bounding box, left zero: unread.
            e[56..58].copy_from_slice(&7u16.to_le_bytes()); // flags, unread
            e[58..60].copy_from_slice(&3u16.to_le_bytes()); // doodadSet
            e[60..62].copy_from_slice(&5u16.to_le_bytes()); // nameSet
            chunk(b"FDOM", &e, &mut out);
        }
        out
    }

    #[test]
    fn reads_the_global_wmo_of_a_wmo_only_map() {
        let wdt = parse(synth_wmo_only_wdt(true, true)).expect("parses");
        let g = wdt.global_wmo().expect("MPHD bit 0 ⇒ a global WMO");
        assert_eq!(g.model, "world\\wmo\\dungeon\\test\\t.wmo");
        assert_eq!(g.position, [11.0, 22.0, 33.0]);
        assert_eq!(g.rotation, [1.0, 180.0, 2.0]);
        assert_eq!((g.doodad_set, g.name_set), (3, 5));
        assert!(!wdt.get_tile(32, 32).expect("in range").has_adt);
    }

    #[test]
    fn an_adt_map_has_no_global_wmo() {
        let wdt = parse(synth_wdt(&[(30, 30)], false)).expect("parses");
        assert!(wdt.global_wmo().is_none());
        // Nor a file with the bit but no placement,
        let half = parse(synth_wmo_only_wdt(true, false)).expect("parses");
        assert!(half.global_wmo().is_none());
        // nor one with the placement but no bit.
        let unflagged = parse(synth_wmo_only_wdt(false, true)).expect("parses");
        assert!(unflagged.global_wmo().is_none());
    }

    #[test]
    fn the_chunk_index_nests_in_the_tile_index() {
        let (ox, oy) = tile_to_world(32, 44);
        for cx in 0..16u32 {
            for cy in 0..16u32 {
                // Chunk (cx, cy) of tile (32, 44): tile_x runs along world y, tile_y along world x.
                let wy = oy - (cx as f32 + 0.5) * CHUNK_YARDS;
                let wx = ox - (cy as f32 + 0.5) * CHUNK_YARDS;
                let (chx, chy) = world_to_chunk(wx, wy);
                assert_eq!((chx >> 4, chy >> 4), world_to_tile(wx, wy));
                assert_eq!((chx, chy), (32 * 16 + cx, 44 * 16 + cy));
            }
        }
        assert_eq!(world_to_chunk(-1.0e6, -1.0e6), (1023, 1023));
        assert_eq!(world_to_chunk(1.0e6, 1.0e6), (0, 0));
    }

    #[test]
    fn world_origin_is_map_center_tile() {
        assert_eq!(world_to_tile(0.0, 0.0), (32, 32));
    }

    #[test]
    fn tile_center_round_trips_exactly() {
        // Tile centres: the half-tile margin absorbs f32 rounding.
        for &(tx, ty) in &[(0, 0), (1, 10), (20, 31), (33, 33), (50, 62), (63, 63)] {
            let (cx, cy) = tile_to_world(tx, ty);
            let center = (cx - 0.5 * T, cy - 0.5 * T);
            assert_eq!(
                world_to_tile(center.0, center.1),
                (tx, ty),
                "tile ({tx},{ty}) center did not round-trip"
            );
        }
    }

    #[test]
    fn tile_to_world_matches_grid_formula() {
        // Max-corner origin: world_x from tile_y, world_y from tile_x (the axis swap).
        assert_eq!(tile_to_world(0, 0), (OFFSET, OFFSET));
        let (wx, wy) = tile_to_world(1, 2);
        assert!((wx - (OFFSET - 2.0 * T)).abs() < 0.01);
        assert!((wy - (OFFSET - 1.0 * T)).abs() < 0.01);
    }

    #[test]
    fn world_to_tile_clamps_both_extremes() {
        assert_eq!(world_to_tile(1.0e9, 1.0e9), (0, 0));
        assert_eq!(world_to_tile(-1.0e9, -1.0e9), (63, 63));
    }

    #[test]
    fn reads_main_grid_and_flags() {
        let wdt = parse(synth_wdt(&[(3, 5), (63, 0), (0, 63)], false)).unwrap();
        assert!(wdt.get_tile(3, 5).unwrap().has_adt);
        assert!(wdt.get_tile(63, 0).unwrap().has_adt);
        assert!(wdt.get_tile(0, 63).unwrap().has_adt);
        let empty = wdt.get_tile(10, 10).unwrap();
        assert!(!empty.has_adt);
        assert_eq!((empty.x, empty.y), (10, 10));
    }

    #[test]
    fn skips_unknown_chunks_before_and_after_main() {
        let wdt = parse(synth_wdt(&[(1, 1)], true)).unwrap();
        assert!(wdt.get_tile(1, 1).unwrap().has_adt);
    }

    #[test]
    fn get_tile_out_of_range_is_none() {
        let wdt = parse(synth_wdt(&[], false)).unwrap();
        assert!(wdt.get_tile(64, 0).is_none());
        assert!(wdt.get_tile(0, 64).is_none());
        assert!(wdt.get_tile(usize::MAX, usize::MAX).is_none());
    }

    #[test]
    fn missing_main_chunk_is_an_error() {
        let mut only_mver = Vec::new();
        only_mver.extend_from_slice(b"REVM");
        only_mver.extend_from_slice(&4u32.to_le_bytes());
        only_mver.extend_from_slice(&18u32.to_le_bytes());
        let err = parse(only_mver).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn empty_stream_has_no_main() {
        // A clean EOF, not a truncation, but still no MAIN.
        let err = parse(Vec::new()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn truncated_chunk_header_is_unexpected_eof() {
        // A stray 3-byte tail cannot form an 8-byte chunk header.
        let err = parse(vec![b'N', b'I', b'A']).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    /// The message pins that the guard refuses, not the read after it.
    #[test]
    fn a_chunk_size_past_the_end_of_stream_is_refused_before_allocating() {
        for magic in [b"DHPM", b"OMWM", b"FDOM"] {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(magic);
            bytes.extend_from_slice(&u32::MAX.to_le_bytes());
            bytes.extend_from_slice(&[0u8; 64]); // some payload, nowhere near the claim
            let err = parse(bytes).unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
            assert_eq!(err.to_string(), "WDT chunk runs past the end of the stream");
        }
    }
}
