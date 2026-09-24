//! Every WMO surface baked near-black at `MOCV.α = 0` that only the doorway fade
//! (`FixColorVertexAlpha`) lights: without the fade the TRANS and INT batches render the bake as
//! is, a black floor. A vertex is dark at luminance < 48 and alpha 0 when the fade lifts it past
//! 200; a triangle counts when all three are, and batches rank by that area.
//! Output is Blizzard data: never commit it.
//! `cargo run --release -p benilla-formats --example wmo_dark_floors [-- <substring>]`

use benilla_wmo::{parse_wmo, ParsedWmo};

/// Rec. 601 luminance of a BGRA slot, in bytes.
fn lum(c: [u8; 4]) -> f32 {
    0.299 * f32::from(c[2]) + 0.587 * f32::from(c[1]) + 0.114 * f32::from(c[0])
}

/// Would this slot have rendered black, and does the fade rescue it?
fn dark(raw: [u8; 4], fixed: [u8; 4]) -> bool {
    raw[3] == 0 && lum(raw) < 48.0 && lum(fixed) > 200.0
}

/// One reported surface: a batch of a group whose bake would not have rendered.
struct Hit {
    wmo: String,
    group: usize,
    batch: usize,
    class: &'static str,
    texture: String,
    /// Square yards of triangles whose every vertex is dark.
    area: f32,
    share: f32,
}

fn main() -> anyhow::Result<()> {
    let filter = std::env::args().nth(1).map(|s| s.to_lowercase());
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;

    // Root .wmo files only: a group file is `<stem>_NNN.wmo`.
    let mut roots: Vec<String> = chain
        .list()?
        .into_iter()
        .map(|e| e.name)
        .filter(|n| {
            let l = n.to_lowercase();
            l.ends_with(".wmo")
                && !l[..l.len() - 4]
                    .rsplit_once('_')
                    .is_some_and(|(_, t)| t.len() == 3 && t.bytes().all(|b| b.is_ascii_digit()))
        })
        .filter(|n| filter.as_ref().is_none_or(|f| n.to_lowercase().contains(f)))
        .collect();
    roots.sort();
    roots.dedup();
    eprintln!("sweeping {} WMO roots…", roots.len());

    let mut hits: Vec<Hit> = Vec::new();
    let (mut scanned, mut groups) = (0usize, 0usize);

    for path in &roots {
        let Ok(root_bytes) = chain.read_file(&path.to_ascii_lowercase()) else {
            continue;
        };
        let Ok(root) = benilla_formats::parse_wmo_root(&root_bytes) else {
            continue;
        };
        let Ok(ParsedWmo::Root(raw_root)) = parse_wmo(&mut std::io::Cursor::new(&root_bytes))
        else {
            continue;
        };
        scanned += 1;
        let stem = &path[..path.len() - 4];
        for gi in 0..root.group_count() as usize {
            let Ok(gbytes) = chain.read_file(&format!("{stem}_{gi:03}.wmo").to_ascii_lowercase())
            else {
                continue;
            };
            let (Some(raw), Some(fixed)) = (
                benilla_formats::wmo_group_raw_colors(&gbytes),
                benilla_formats::wmo_group_fixed_colors(&gbytes, &root),
            ) else {
                continue;
            };
            let Ok(ParsedWmo::Group(group)) = parse_wmo(&mut std::io::Cursor::new(&gbytes)) else {
                continue;
            };
            groups += 1;
            let is_dark: Vec<bool> = raw.iter().zip(&fixed).map(|(r, f)| dark(*r, *f)).collect();
            if !is_dark.iter().any(|&d| d) {
                continue;
            }
            // MOBA runs TRANS, INT, EXT; only the first two render the bake.
            let (trans_n, int_n) = benilla_formats::wmo_group_header(&gbytes)
                .map(|_| ())
                .and_then(|()| {
                    let m = mogp(&gbytes)?;
                    (m.len() >= 0x2c).then(|| {
                        (
                            u16::from_le_bytes([m[0x28], m[0x29]]) as usize,
                            u16::from_le_bytes([m[0x2a], m[0x2b]]) as usize,
                        )
                    })
                })
                .unwrap_or((0, 0));
            for (bi, batch) in group.render_batches.iter().enumerate() {
                let class = if bi < trans_n {
                    "TRANS"
                } else if bi < trans_n + int_n {
                    "INT"
                } else {
                    continue; // EXT batches take the exterior law; the bake never shows
                };
                let start = batch.start_index as usize;
                let idx = group
                    .vertex_indices
                    .get(start..start + batch.count as usize)
                    .unwrap_or(&[]);
                let (mut dark_area, mut total_area) = (0.0f32, 0.0f32);
                for t in idx.as_chunks::<3>().0 {
                    let p: Vec<[f32; 3]> = t
                        .iter()
                        .filter_map(|&i| group.vertex_positions.get(i as usize))
                        .map(|v| [v.x, v.y, v.z])
                        .collect();
                    if p.len() != 3 {
                        continue;
                    }
                    let (u, v) = (
                        [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]],
                        [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]],
                    );
                    let c = [
                        u[1] * v[2] - u[2] * v[1],
                        u[2] * v[0] - u[0] * v[2],
                        u[0] * v[1] - u[1] * v[0],
                    ];
                    let a = 0.5 * (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
                    total_area += a;
                    if t.iter()
                        .all(|&i| is_dark.get(i as usize).copied() == Some(true))
                    {
                        dark_area += a;
                    }
                }
                // A quad of black floor is ~10 yd²; below 2 yd² is a seam, not a surface.
                if dark_area < 2.0 {
                    continue;
                }
                let material = raw_root.materials.get(batch.material_id as usize);
                let texture = material
                    .map(|m| m.get_texture1_index(&raw_root.texture_offset_index_map))
                    .and_then(|i| raw_root.textures.get(i as usize))
                    .map(|t| t.rsplit('\\').next().unwrap_or(t).to_string())
                    .unwrap_or_else(|| "(none)".into());
                hits.push(Hit {
                    wmo: path.clone(),
                    group: gi,
                    batch: bi,
                    class,
                    texture,
                    area: dark_area,
                    share: dark_area / total_area.max(1e-6),
                });
            }
        }
    }

    hits.sort_by(|a, b| b.area.total_cmp(&a.area));
    println!("\n=== Surfaces that rendered black before the doorway fade (by area) ===\n");
    for h in &hits {
        println!(
            "{:9.1} yd²  {:3.0}%  g{:03} b{:<3} {:<5} {:<28} {}",
            h.area,
            h.share * 100.0,
            h.group,
            h.batch,
            h.class,
            h.texture,
            h.wmo
        );
    }
    let buildings: std::collections::BTreeSet<&str> = hits.iter().map(|h| h.wmo.as_str()).collect();
    println!(
        "\n{} surface(s) across {} building(s) of {scanned} scanned ({groups} MOCV groups); \
         {:.0} yd² of unrenderable surface in total",
        hits.len(),
        buildings.len(),
        hits.iter().map(|h| h.area).sum::<f32>()
    );
    Ok(())
}

/// The MOGP super-chunk payload of a group file.
fn mogp(bytes: &[u8]) -> Option<&[u8]> {
    let mut off = 0usize;
    while off + 8 <= bytes.len() {
        let len = u32::from_le_bytes([
            bytes[off + 4],
            bytes[off + 5],
            bytes[off + 6],
            bytes[off + 7],
        ]) as usize;
        if &bytes[off..off + 4] == b"PGOM" {
            return bytes.get(off + 8..(off + 8 + len).min(bytes.len()));
        }
        off += 8 + len;
    }
    None
}
