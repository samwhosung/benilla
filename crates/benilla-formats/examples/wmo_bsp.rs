//! Which WMO faces the reference's collision BSP can reach, against the faces we collide.
//! `cargo run -p benilla-formats --example wmo_bsp -- <wmo-path-or-substring>`
//!
//! The reference tests only the faces a group's MOBN leaves list in MOBR, skipping one iff
//! `MOPY.flags & rejectMask != 0`: walking's box query at leaf `0x6bca50` with mask `0x84`, the
//! camera and LOS segment query at leaf `0x6bc700` with `0x82`. `accumulate_wmo_group_faces`
//! walks every face the mask keeps, so the two agree only where MOBR reaches them all, as it does
//! on CavernsOfTime. `inward` counts faces turned toward the centroid: with MOMT `0x04` clear the
//! reference culls back faces, so a hull built to be seen from inside is see-through from outside.
//! Output is Blizzard data: never commit it.

use std::collections::HashSet;

use benilla_wmo::{parse_wmo, ParsedWmo};

/// Walking's persistent MOPY reject bit (DETAIL): mask `0x84` (`0x6315f0`) minus the transient
/// `0x80` visited bit.
const MOPY_DETAIL: u8 = 0x04;

fn main() -> anyhow::Result<()> {
    let pat = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: wmo_bsp <wmo-path-or-substring>"))?;
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let path = if pat.to_lowercase().ends_with(".wmo") {
        pat.clone()
    } else {
        let lower = pat.to_lowercase();
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
    println!("{path}\n");

    let root_bytes = chain.read_file(&path.to_ascii_lowercase())?;
    let ParsedWmo::Root(root) = parse_wmo(&mut std::io::Cursor::new(&root_bytes))? else {
        anyhow::bail!("{path} is a group file, want the root");
    };
    let unculled = root
        .materials
        .iter()
        .filter(|m| m.flags & 0x04 != 0)
        .count();
    println!(
        "=== MOMT: {unculled} of {} materials carry UNCULLED (0x04) ===",
        root.materials.len()
    );
    let stem = path.to_ascii_lowercase();
    let stem = stem.strip_suffix(".wmo").unwrap_or(&stem);

    println!("\n=== per group: what the BSP reaches vs what we collide ===");
    println!(
        "{:>4} {:>10} {:>7} {:>8} {:>8} {:>9} {:>9} {:>6} gap (we−bsp)",
        "grp", "mogpFlags", "faces", "MOBNn", "MOBRn", "bspFaces", "weCollide", "inward",
    );
    let (mut tot_faces, mut tot_bsp, mut tot_we, mut tot_gap) = (0usize, 0usize, 0usize, 0usize);
    for gi in 0..root.n_groups {
        let group_path = format!("{stem}_{gi:03}.wmo");
        let Ok(gbytes) = chain.read_file(&group_path) else {
            continue;
        };
        let ParsedWmo::Group(group) = parse_wmo(&mut std::io::Cursor::new(&gbytes))? else {
            continue;
        };
        let n_faces = group.vertex_indices.len() / 3;
        let nodes = mogp_subchunk(&gbytes, b"NBOM").unwrap_or(&[]);
        let mobn = nodes.len() / 0x10;
        let mobr: Vec<u16> = mogp_subchunk(&gbytes, b"RBOM")
            .map(|c| {
                c.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                    .collect()
            })
            .unwrap_or_default();
        // Reachability, not presence: a face under a leaf the descent never enters is never tested.
        let (bsp_reach, bad) = descend(nodes, &mobr);
        // Walking in the reference hits reachable faces that clear DETAIL; ours hits all that do.
        let (mut bsp_hit, mut we_hit, mut gap) = (0usize, 0usize, 0usize);
        let mut gap_aabb = [[f32::MAX; 3], [f32::MIN; 3]];
        // Faces whose winding normal points at the group's centroid, over every face.
        let mut centroid = [0.0f64; 3];
        for p in &group.vertex_positions {
            for (c, v) in [p.x, p.y, p.z].into_iter().enumerate() {
                centroid[c] += f64::from(v);
            }
        }
        let n_v = group.vertex_positions.len().max(1) as f64;
        let centroid = centroid.map(|c| (c / n_v) as f32);
        let mut inward = 0usize;
        for f in 0..n_faces {
            if let Some(n) = winding_normal(&group, f) {
                let c = face_centroid(&group, f).unwrap_or([0.0; 3]);
                let to_mid = [centroid[0] - c[0], centroid[1] - c[1], centroid[2] - c[2]];
                if n[0] * to_mid[0] + n[1] * to_mid[1] + n[2] * to_mid[2] > 0.0 {
                    inward += 1;
                }
            }
            let flags = group.material_info.get(f).map_or(0, |m| m.flags);
            if flags & MOPY_DETAIL != 0 {
                continue;
            }
            we_hit += 1;
            if bsp_reach.contains(&(f as u16)) {
                bsp_hit += 1;
            } else {
                gap += 1;
                for k in 0..3 {
                    let Some(&vi) = group.vertex_indices.get(f * 3 + k) else {
                        continue;
                    };
                    let Some(p) = group.vertex_positions.get(vi as usize) else {
                        continue;
                    };
                    for (c, v) in [p.x, p.y, p.z].into_iter().enumerate() {
                        gap_aabb[0][c] = gap_aabb[0][c].min(v);
                        gap_aabb[1][c] = gap_aabb[1][c].max(v);
                    }
                }
            }
        }
        tot_faces += n_faces;
        tot_bsp += bsp_hit;
        tot_we += we_hit;
        tot_gap += gap;
        let note = if gap > 0 {
            format!(
                "{gap:>6}   aabb ({:.0},{:.0},{:.0})…({:.0},{:.0},{:.0})",
                gap_aabb[0][0],
                gap_aabb[0][1],
                gap_aabb[0][2],
                gap_aabb[1][0],
                gap_aabb[1][1],
                gap_aabb[1][2],
            )
        } else {
            "     0".to_string()
        };
        println!(
            "{gi:>4} {:>#10x} {n_faces:>7} {mobn:>8} {:>8} {bsp_hit:>9} {we_hit:>9} {:>6} {note}{}",
            group.flags,
            mobr.len(),
            format!("{:.0}%", 100.0 * inward as f32 / n_faces.max(1) as f32),
            if bad > 0 {
                format!("  [!! {bad} malformed node refs — layout suspect]")
            } else {
                String::new()
            },
        );
    }
    println!(
        "\ntotals: {tot_faces} faces · bsp-reachable+walkable {tot_bsp} · we collide {tot_we} · \
         gap {tot_gap}"
    );
    Ok(())
}

fn face_verts(group: &benilla_wmo::WmoGroup, f: usize) -> Option<[[f32; 3]; 3]> {
    let mut out = [[0.0f32; 3]; 3];
    for (k, slot) in out.iter_mut().enumerate() {
        let vi = *group.vertex_indices.get(f * 3 + k)? as usize;
        let p = group.vertex_positions.get(vi)?;
        *slot = [p.x, p.y, p.z];
    }
    Some(out)
}

/// A face's right-hand normal in its authored winding: the side it presents under the reference's
/// CCW back-face cull, and ours, since `wow_to_bevy` is a proper rotation.
fn winding_normal(group: &benilla_wmo::WmoGroup, f: usize) -> Option<[f32; 3]> {
    let [a, b, c] = face_verts(group, f)?;
    let (u, v) = (
        [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
        [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
    );
    Some([
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ])
}

fn face_centroid(group: &benilla_wmo::WmoGroup, f: usize) -> Option<[f32; 3]> {
    let vs = face_verts(group, f)?;
    let mut out = [0.0f32; 3];
    for (c, slot) in out.iter_mut().enumerate() {
        *slot = (vs[0][c] + vs[1][c] + vs[2][c]) / 3.0;
    }
    Some(out)
}

/// The MOBR face indices a descent from MOBN node 0 can reach, and the count of out-of-range
/// children or spans, which a misread layout drives up at once. A node is `u16 flags, i16 negChild,
/// i16 posChild, u16 nFaces, u32 faceStart, f32 planeDist`, stride `0x10` (`0x692f20`), and
/// `flags & 0x4` marks a leaf.
fn descend(nodes: &[u8], mobr: &[u16]) -> (HashSet<u16>, usize) {
    let n = nodes.len() / 0x10;
    let (mut seen, mut reach, mut bad) = (vec![false; n], HashSet::new(), 0usize);
    let mut stack = if n > 0 { vec![0i16] } else { Vec::new() };
    while let Some(i) = stack.pop() {
        let Ok(u) = usize::try_from(i) else { continue };
        if u >= n {
            bad += 1;
            continue;
        }
        if std::mem::replace(&mut seen[u], true) {
            continue;
        }
        let b = &nodes[u * 0x10..u * 0x10 + 0x10];
        let rd16 = |o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
        let flags = u16::from_le_bytes([b[0], b[1]]);
        if flags & 0x4 != 0 {
            let n_faces = u16::from_le_bytes([b[6], b[7]]) as usize;
            let start = u32::from_le_bytes([b[8], b[9], b[10], b[11]]) as usize;
            match mobr.get(start..start + n_faces) {
                Some(span) => reach.extend(span.iter().copied()),
                None => bad += 1,
            }
        } else {
            for c in [rd16(2), rd16(4)] {
                if c >= 0 {
                    stack.push(c);
                }
            }
        }
    }
    (reach, bad)
}

/// Find a sub-chunk of a group file's MOGP super-chunk by on-disk (reversed) magic.
fn mogp_subchunk<'a>(group_bytes: &'a [u8], magic: &[u8; 4]) -> Option<&'a [u8]> {
    let mogp = top_chunk(group_bytes, b"PGOM")?;
    let mut off = 0x44usize;
    while off + 8 <= mogp.len() {
        let size = u32::from_le_bytes(mogp[off + 4..off + 8].try_into().ok()?) as usize;
        let (start, end) = (off + 8, off + 8 + size);
        if end > mogp.len() {
            break;
        }
        if &mogp[off..off + 4] == magic {
            return Some(&mogp[start..end]);
        }
        off = end;
    }
    None
}

/// Find a top-level chunk by on-disk (reversed) magic.
fn top_chunk<'a>(bytes: &'a [u8], magic: &[u8; 4]) -> Option<&'a [u8]> {
    let mut off = 0usize;
    while off + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[off + 4..off + 8].try_into().ok()?) as usize;
        let (start, end) = (off + 8, off + 8 + size);
        if end > bytes.len() {
            break;
        }
        if &bytes[off..off + 4] == magic {
            return Some(&bytes[start..end]);
        }
        off = end;
    }
    None
}
