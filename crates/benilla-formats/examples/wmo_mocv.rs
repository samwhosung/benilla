//! Each WMO group's MOCV bake per batch, up-facing vertices apart from the rest, with its MOGP
//! flags, batch class and the bake after the doorway fade. The reference lights an interior surface
//! only from the bake: an INT batch draws `tex × MOCV` and a TRANS batch lerps to it by `MOCV.a`,
//! and `FixColorVertexAlpha` (`0x6c43d0`) brightens interior vertices toward white within 6.67 yd
//! of a portal to an exterior group. A model-space `x,y,z` keeps only the groups whose box holds
//! it; `--survey` prints only the fade per group. Output is Blizzard data: never commit it.
//! `cargo run -p benilla-formats --example wmo_mocv -- <wmo-path-or-substring> [gN | x,y,z]`

use benilla_wmo::{parse_wmo, ParsedWmo};

/// The payload of a WMO file's top-level chunk `magic`, FourCC reversed as on disk.
fn chunk<'a>(bytes: &'a [u8], magic: &[u8; 4]) -> Option<&'a [u8]> {
    let mut off = 0usize;
    while off + 8 <= bytes.len() {
        let id = &bytes[off..off + 4];
        let len = u32::from_le_bytes([
            bytes[off + 4],
            bytes[off + 5],
            bytes[off + 6],
            bytes[off + 7],
        ]) as usize;
        let body = off + 8;
        if id == magic {
            return bytes.get(body..(body + len).min(bytes.len()));
        }
        off = body + len;
    }
    None
}

/// The MOCV spread of one set of vertices, in bytes.
struct Stat {
    n: usize,
    lum_min: f32,
    lum_max: f32,
    sum: [f32; 4],
}

impl Stat {
    fn new() -> Self {
        Self {
            n: 0,
            lum_min: f32::MAX,
            lum_max: f32::MIN,
            sum: [0.0; 4],
        }
    }
    fn add(&mut self, c: [f32; 4]) {
        let lum = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
        self.n += 1;
        self.lum_min = self.lum_min.min(lum);
        self.lum_max = self.lum_max.max(lum);
        for (i, ch) in c.iter().enumerate() {
            self.sum[i] += ch;
        }
    }
    fn line(&self) -> String {
        if self.n == 0 {
            return "—".into();
        }
        let k = self.n as f32;
        format!(
            "n={:<5} mean rgba ({:3.0},{:3.0},{:3.0},{:3.0})  lum {:3.0}..{:3.0}",
            self.n,
            self.sum[0] / k,
            self.sum[1] / k,
            self.sum[2] / k,
            self.sum[3] / k,
            self.lum_min,
            self.lum_max,
        )
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let pat = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: wmo_mocv <wmo-path-or-substring> [gN | x,y,z]"))?;
    let filter = args.next();
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    // A `.wmo` argument is a literal path only if the chain has it; `stormwind.wmo` is a substring.
    let lower = pat.to_lowercase();
    let path = if lower.ends_with(".wmo") && chain.read_file(&lower).is_ok() {
        pat.clone()
    } else {
        chain
            .list()?
            .into_iter()
            .map(|e| e.name)
            .find(|n| {
                let l = n.to_lowercase();
                l.ends_with(".wmo") && l.contains(&lower) && !l.contains("_00")
            })
            .ok_or_else(|| anyhow::anyhow!("no root .wmo matching {pat:?}"))?
    };
    println!("{path}");

    let root_bytes = chain.read_file(&path.to_ascii_lowercase())?;
    let ParsedWmo::Root(root) = parse_wmo(&mut std::io::Cursor::new(&root_bytes))? else {
        anyhow::bail!("{path} is a group file, want the root");
    };
    // MOHD: nTextures, nGroups, nPortals, nLights, nDoodadNames, nDoodadDefs, nDoodadSets (7×u32),
    // then ambColor @0x1c, wmoID @0x20, bbox @0x24 (24 B), flags @0x3c.
    if let Some(h) = chunk(&root_bytes, b"DHOM") {
        let at = |i: usize| u32::from_le_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]]);
        println!(
            "MOHD: {} groups, {} portals, {} lights, {} doodad defs  ambColor {:#010x}  \
             wmoID {}  flags {:#06x}",
            at(4),
            at(8),
            at(0xc),
            at(0x14),
            at(0x1c),
            at(0x20),
            if h.len() >= 0x40 { at(0x3c) } else { 0 },
        );
    }
    println!(
        "{} materials, {} groups",
        root.materials.len(),
        root.n_groups
    );

    let wmo_root = benilla_formats::parse_wmo_root(&root_bytes)?;
    let survey = filter.as_deref() == Some("--survey");

    let want_group: Option<usize> = filter
        .as_deref()
        .and_then(|f| f.strip_prefix('g'))
        .and_then(|n| n.parse().ok());
    let want_point: Option<[f32; 3]> =
        filter.as_deref().filter(|f| f.contains(',')).and_then(|f| {
            let v: Vec<f32> = f.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        });

    let stem = &path[..path.len() - 4];
    for gi in 0..root.n_groups as usize {
        if want_group.is_some_and(|w| w != gi) {
            continue;
        }
        let gpath = format!("{stem}_{gi:03}.wmo");
        let Ok(gbytes) = chain.read_file(&gpath.to_ascii_lowercase()) else {
            continue;
        };
        let Ok(ParsedWmo::Group(group)) = parse_wmo(&mut std::io::Cursor::new(&gbytes)) else {
            continue;
        };
        // MOGP header: flags @0x08, bbox min @0x0c / max @0x18, portal span @0x24/0x26, and the
        // batch-section counts TRANS @0x28 / INT @0x2a (MOBA is laid out TRANS, INT, EXT).
        let Some(mogp) = chunk(&gbytes, b"PGOM").filter(|m| m.len() >= 0x2c) else {
            continue;
        };
        let f32_at =
            |i: usize| f32::from_le_bytes([mogp[i], mogp[i + 1], mogp[i + 2], mogp[i + 3]]);
        let bb = [
            f32_at(0x0c),
            f32_at(0x10),
            f32_at(0x14),
            f32_at(0x18),
            f32_at(0x1c),
            f32_at(0x20),
        ];
        if let Some(p) = want_point {
            if !(0..3).all(|i| p[i] >= bb[i] - 1.0 && p[i] <= bb[i + 3] + 1.0) {
                continue;
            }
        }
        let trans_n = u16::from_le_bytes([mogp[0x28], mogp[0x29]]) as usize;
        let int_n = u16::from_le_bytes([mogp[0x2a], mogp[0x2b]]) as usize;
        let interior = (group.flags & 0x48) == 0;
        let has_colors = group.vertex_colors.len() == group.vertex_positions.len();
        println!(
            "\n=== g{gi:03} flags {:#010x} {}  portals {}  bbox ({:.0},{:.0},{:.0})..\
             ({:.0},{:.0},{:.0})  {} verts  MOCV {}  {} batches (trans {trans_n}, int {int_n})",
            group.flags,
            if interior { "INTERIOR" } else { "exterior" },
            u16::from_le_bytes([mogp[0x26], mogp[0x27]]),
            bb[0],
            bb[1],
            bb[2],
            bb[3],
            bb[4],
            bb[5],
            group.vertex_positions.len(),
            if has_colors { "yes" } else { "ABSENT" },
            group.render_batches.len(),
        );
        // How many authored slots the doorway fade rewrites on this group, and by how much.
        if let Some(fixed) = benilla_formats::wmo_group_fixed_colors(&gbytes, &wmo_root) {
            let lum = |c: [f32; 3]| 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
            let (mut changed, mut before, mut after) = (0usize, 0.0f32, 0.0f32);
            for (raw, f) in group.vertex_colors.iter().zip(&fixed) {
                let (b, a) = (
                    lum([raw.r as f32, raw.g as f32, raw.b as f32]),
                    lum([f[2] as f32, f[1] as f32, f[0] as f32]),
                );
                before += b;
                after += a;
                if [raw.b, raw.g, raw.r, raw.a] != *f {
                    changed += 1;
                }
            }
            let n = fixed.len().max(1) as f32;
            println!(
                "    doorway fade: {changed}/{} verts rewritten, mean lum {:.0} → {:.0}",
                fixed.len(),
                before / n,
                after / n
            );
        }
        if survey {
            continue;
        }
        for (bi, batch) in group.render_batches.iter().enumerate() {
            let class = if bi < trans_n {
                "TRANS"
            } else if bi < trans_n + int_n {
                "INT"
            } else {
                "EXT"
            };
            let material = root.materials.get(batch.material_id as usize);
            let tex = material
                .map(|m| m.get_texture1_index(&root.texture_offset_index_map))
                .and_then(|i| root.textures.get(i as usize))
                .map(String::as_str)
                .unwrap_or("(none)");
            let start = batch.start_index as usize;
            let idx = group
                .vertex_indices
                .get(start..start + batch.count as usize)
                .unwrap_or(&[]);
            // Split by facing, so a floor's up-facing verts read apart from its walls.
            let (mut up, mut other) = (Stat::new(), Stat::new());
            for &i in idx {
                let i = i as usize;
                let Some(c) = group.vertex_colors.get(i) else {
                    continue;
                };
                let c = [c.r as f32, c.g as f32, c.b as f32, c.a as f32];
                if group.vertex_normals.get(i).is_some_and(|n| n.z > 0.7) {
                    up.add(c);
                } else {
                    other.add(c);
                }
            }
            println!(
                "  b{bi:3} {class:<5} mat{:<3} flags {:#06x} blend {}  {tex}",
                batch.material_id,
                material.map_or(0, |m| m.flags),
                material.map_or(0, |m| m.blend_mode),
            );
            println!("        up-facing {}", up.line());
            println!("        other     {}", other.line());
        }
        // The same census after the reader's doorway fade, as the renderer uploads it: black above
        // and white here is a surface only the fade lights.
        let subs = benilla_formats::wmo_group_submeshes(&gbytes, &wmo_root)?;
        println!(
            "  --- after FixColorVertexAlpha ({} submeshes) ---",
            subs.len()
        );
        for (si, s) in subs.iter().enumerate() {
            let (mut up, mut other) = (Stat::new(), Stat::new());
            for (vi, c) in s.vertex_colors.iter().enumerate() {
                let c = c.map(|v| v * 255.0);
                if s.normals.get(vi).is_some_and(|n| n[2] > 0.7) {
                    up.add(c);
                } else {
                    other.add(c);
                }
            }
            println!(
                "  s{si:3} {}  up-facing {}",
                s.texture.as_deref().unwrap_or("(none)"),
                up.line()
            );
            println!("        other     {}", other.line());
        }
    }
    Ok(())
}
