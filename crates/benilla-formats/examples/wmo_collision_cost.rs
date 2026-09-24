//! Each WMO's collision triangle counts, walking against camera gather, and the groups whose
//! walking gather (MOPY DETAIL `0x04` dropped) has no walkable face where the camera's does. The
//! triangle count drives how long a placement takes to turn solid, against the mover's
//! `SETTLE_TIMEOUT`. Output is Blizzard data: never commit it.
//! `cargo run -p benilla-formats --example wmo_collision_cost -- <wmo-path-or-substring>...`

use benilla_formats::{accumulate_wmo_group_camera_collision, accumulate_wmo_group_collision};

/// A face at least this upward-facing is a floor, as the mover's `GROUND_COS` (about 50° from
/// vertical) judges it.
const WALKABLE_COS: f32 = 0.64;

struct Gather {
    tris: usize,
    walkable: usize,
    min_z: f32,
    max_z: f32,
}

fn gather(positions: &[[f32; 3]], indices: &[u32]) -> Gather {
    let (mut walkable, mut min_z, mut max_z) = (0usize, f32::MAX, f32::MIN);
    for t in indices.as_chunks::<3>().0 {
        let p: [[f32; 3]; 3] = [
            positions[t[0] as usize],
            positions[t[1] as usize],
            positions[t[2] as usize],
        ];
        let u = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
        let v = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
        // Raw WMO space is Z-up, so the walkable axis is the normal's Z.
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 && (n[2] / len).abs() >= WALKABLE_COS {
            walkable += 1;
        }
        for c in p {
            min_z = min_z.min(c[2]);
            max_z = max_z.max(c[2]);
        }
    }
    Gather {
        tris: indices.len() / 3,
        walkable,
        min_z,
        max_z,
    }
}

fn main() -> anyhow::Result<()> {
    let pats: Vec<String> = std::env::args().skip(1).collect();
    if pats.is_empty() {
        anyhow::bail!("usage: wmo_collision_cost <wmo-path-or-substring>...");
    }
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let names: Vec<String> = chain.list()?.into_iter().map(|e| e.name).collect();

    for pat in &pats {
        let lower = pat.to_lowercase();
        let path = if lower.ends_with(".wmo") && chain.contains(pat) {
            pat.clone()
        } else {
            let Some(found) = names.iter().find(|n| {
                let l = n.to_lowercase();
                l.ends_with(".wmo")
                    && l.contains(&lower)
                    && !l.rsplit_once('_').is_some_and(|(_, tail)| {
                        tail.len() == 7 && tail.starts_with(|c: char| c.is_ascii_digit())
                    })
            }) else {
                println!("no root .wmo matching {pat:?}\n");
                continue;
            };
            found.clone()
        };

        let bytes = chain.read_file(&path.to_ascii_lowercase())?;
        let n_groups = benilla_formats::parse_wmo_root(&bytes)?.group_count();
        let stem = {
            let l = path.to_ascii_lowercase();
            l.strip_suffix(".wmo").unwrap_or(&l).to_string()
        };

        println!("=== {path} — {n_groups} group(s) ===");
        let (mut tot_walk, mut tot_cam, mut floorless) = (0usize, 0usize, Vec::new());
        for gi in 0..n_groups {
            let Ok(gbytes) = chain.read_file(&format!("{stem}_{gi:03}.wmo")) else {
                continue;
            };
            let (mut wp, mut wi) = (Vec::new(), Vec::new());
            accumulate_wmo_group_collision(&gbytes, &mut wp, &mut wi);
            let (mut cp, mut ci) = (Vec::new(), Vec::new());
            accumulate_wmo_group_camera_collision(&gbytes, &mut cp, &mut ci);
            let (w, c) = (gather(&wp, &wi), gather(&cp, &ci));
            tot_walk += w.tris;
            tot_cam += c.tris;
            // The defect shape: the camera can stand on this group, the player body cannot.
            if w.walkable == 0 && c.walkable > 0 {
                floorless.push(gi);
                println!(
                    "  group {gi:3}: walk {:6} tri ({:5} walkable)   camera {:6} tri ({:5} walkable)  \
                     <-- NO WALKABLE FACE IN THE WALK GATHER (z {:.1}..{:.1})",
                    w.tris, w.walkable, c.tris, c.walkable, c.min_z, c.max_z
                );
            }
        }
        println!(
            "  TOTAL walk {tot_walk} tri, camera {tot_cam} tri  \
             ({} group(s) with a camera-only floor)\n",
            floorless.len()
        );
    }
    Ok(())
}
