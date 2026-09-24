//! WMO (building) parsing, split along the file boundary the format itself draws: [`root`] parses
//! the root file's shared tables (materials, doodads, group infos, portals, fogs), [`group`] parses
//! a group file (header, refs, render batches) against a parsed root. This module keeps the shared
//! chunk-finder and the chain-reading entry point.

mod group;
mod root;

pub use group::*;
pub use root::*;

use anyhow::{Context, Result};

use crate::Chain;

/// Load a WMO (building) from the chain: its root + every group file, flattened into submeshes
/// (one per group render batch). `raw_path` is the MWMO root path (`.wmo`).
pub fn load_wmo(chain: &mut Chain, raw_path: &str) -> Result<Vec<super::RenderSubmesh>> {
    let root_path = raw_path.to_ascii_lowercase();
    let bytes = chain
        .read_file(&root_path)
        .with_context(|| format!("reading WMO {root_path}"))?;
    let root = parse_wmo_root(&bytes).with_context(|| format!("parsing WMO {root_path}"))?;
    let stem = root_path.strip_suffix(".wmo").unwrap_or(&root_path);
    let mut out = Vec::new();
    for gi in 0..root.group_count() {
        let group_path = format!("{stem}_{gi:03}.wmo");
        let Ok(gbytes) = chain.read_file(&group_path) else {
            continue;
        };
        out.extend(wmo_group_submeshes(&gbytes, &root)?);
    }
    Ok(out)
}

/// Find a top-level WMO chunk's data slice by its **on-disk (reversed) magic** — WMO/ADT store the
/// FourCC reversed (`MODN` → `NDOM`). Top-level root chunks are laid out flat from byte 0
/// (`[magic:4][size:u32 LE][data:size]`), so a linear walk locates any of them.
///
/// **The last chunk clamps to EOF; it never rejects the file.** The reference's walk
/// (`0x6c3a60`/`0x6c3f80`) "reads chunks while the 8-byte header is in-bounds and clamps the last
/// chunk to EOF (never requires exact tiling)", and its worked example is the file this rule exists
/// for: `Undercity_144.wmo`'s MOGP declares one byte more than the file holds.
/// Abandoning the walk there cost that group its MOGP entirely — flags, portal-ref span, area, fog,
/// doodad and light refs — which dead-ended the portal flood at B26's doorway.
pub(crate) fn find_wmo_chunk<'a>(bytes: &'a [u8], magic: &[u8; 4]) -> Option<&'a [u8]> {
    let mut off = 0usize;
    while off + 8 <= bytes.len() {
        let size = u32::from_le_bytes([
            bytes[off + 4],
            bytes[off + 5],
            bytes[off + 6],
            bytes[off + 7],
        ]) as usize;
        let data_start = off + 8;
        // Saturating, then clamped: a garbage size can overflow, and an over-declared one is real
        // data the reference tolerates. Either way `data_end >= data_start`, so the slice is valid
        // and `off` still advances by at least 8 — the walk terminates.
        let data_end = data_start.saturating_add(size).min(bytes.len());
        if &bytes[off..off + 4] == magic {
            return Some(&bytes[data_start..data_end]);
        }
        off = data_end;
    }
    None
}

/// Synthetic-chunk builders shared by the [`root`]/[`group`] test modules.
#[cfg(test)]
mod test_bytes {
    /// Build one top-level WMO chunk: the FourCC stored **reversed**, a `u32 LE` size, then `data`.
    pub(super) fn chunk(reversed_magic: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + data.len());
        out.extend_from_slice(reversed_magic);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    pub(super) fn f3(v: [f32; 3]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }
}
