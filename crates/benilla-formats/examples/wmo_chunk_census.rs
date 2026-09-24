//! Chunk-integrity census over WMO files: walk the top-level chunk list the way the loader's
//! [`find_wmo_chunk`] walk does and report every file whose chunk stream does not tile exactly — a
//! chunk whose declared size overruns EOF. The reference **clamps** such a chunk (`0x6c3a60` /
//! `0x6c3f80` read chunks while the 8-byte header is in-bounds and clamp the last chunk to EOF —
//! e.g. `Undercity_144.wmo`'s MOGP runs 1 B past EOF, tolerated), so an overrun here must cost us
//! nothing; a file listed as losing a chunk is a bug.
//!
//! ```text
//! cargo run -p benilla-formats --example wmo_chunk_census -- <root.wmo>   # one building
//! cargo run -p benilla-formats --example wmo_chunk_census -- --all        # the whole corpus
//! ```
//!
//! Output is Blizzard-derived — pipe it to the scratchpad, never into the repo.

/// Walk `b`'s chunk stream as a pure **data** question: is it exactly tiled, or does some chunk
/// declare more bytes than the file holds? Returns `(tag, declared_end, file_len)` for the first
/// over-declared chunk. Whether the loader survives it is asked separately, of the loader itself.
fn overrun(b: &[u8]) -> Option<(String, usize, usize)> {
    let mut off = 0usize;
    while off + 8 <= b.len() {
        let magic: String = b[off..off + 4].iter().rev().map(|&c| c as char).collect();
        let size = u32::from_le_bytes([b[off + 4], b[off + 5], b[off + 6], b[off + 7]]) as usize;
        let end = match off.checked_add(8).and_then(|s| s.checked_add(size)) {
            Some(e) => e,
            None => return Some((magic, usize::MAX, b.len())),
        };
        if end > b.len() {
            return Some((magic, end, b.len()));
        }
        off = end;
    }
    None
}

fn main() -> anyhow::Result<()> {
    let arg = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: wmo_chunk_census <root.wmo> | --all"))?;
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;

    let paths: Vec<String> = if arg == "--all" {
        let mut v: Vec<String> = chain
            .list()?
            .into_iter()
            .map(|e| e.name)
            .filter(|n| n.to_ascii_lowercase().ends_with(".wmo"))
            .collect();
        v.sort();
        v.dedup();
        v
    } else {
        let root = chain.read_file(&arg)?;
        let root = benilla_formats::parse_wmo_root(&root)?;
        let stem = arg.strip_suffix(".wmo").unwrap_or(&arg);
        std::iter::once(arg.clone())
            .chain((0..root.group_count()).map(|gi| format!("{stem}_{gi:03}.wmo")))
            .collect()
    };

    let (mut checked, mut empty, mut overrun_n, mut lost) = (0u32, 0u32, 0u32, 0u32);
    for p in &paths {
        let Ok(b) = chain.read_file(p) else { continue };
        if b.is_empty() {
            empty += 1;
            continue;
        }
        checked += 1;
        // The loader's own verdict, not a re-implementation of it: a group file must yield its MOGP
        // header (flags + portal-ref span — losing it dead-ends the portal flood), a root must parse.
        let is_group = b.len() > 16 && &b[12..16] == b"PGOM";
        let loads = if is_group {
            benilla_formats::wmo_group_header(&b).is_some()
        } else {
            benilla_formats::parse_wmo_root(&b).is_ok()
        };
        if !loads {
            lost += 1;
            println!("{p}: the loader gets NO {} out of it", {
                if is_group {
                    "MOGP header"
                } else {
                    "root"
                }
            });
        }
        if let Some((tag, end, len)) = overrun(&b) {
            overrun_n += 1;
            println!(
                "{p}: {tag} declares end {end}, file is {len} ({} over) — loader {}",
                end.saturating_sub(len),
                if loads {
                    "tolerates it"
                } else {
                    "LOSES THE CHUNK"
                }
            );
        }
    }
    println!(
        "{checked} wmo files walked ({empty} empty stubs): {overrun_n} with an over-declared chunk, \
         {lost} the loader cannot read"
    );
    Ok(())
}
