//! WMO (building) parsing: [`root`] parses the root file's shared tables, [`group`] a group file
//! against a parsed root.

mod group;
mod root;

pub use group::*;
pub use root::*;

use anyhow::{Context, Result};

use crate::Chain;

/// Load a WMO's root and every group file from the chain, one submesh per group render batch.
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

/// Find a top-level WMO chunk's data by its on-disk magic, which WMO stores reversed (`MODN` is
/// `NDOM`). As in the reference's walk (`0x6c3a60`/`0x6c3f80`), the last chunk clamps to EOF and
/// never rejects the file: `Undercity_144.wmo`'s MOGP declares one byte more than the file holds.
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
        // Saturating, then clamped: `data_end >= data_start` for any size, so the walk advances.
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
    /// One top-level WMO chunk: the reversed FourCC, a `u32` LE size, then `data`.
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
