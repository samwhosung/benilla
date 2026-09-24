//! Which batches of a WMO are coplanar, with each other or with themselves: per pair, the smallest
//! plane gap between near-parallel overlapping faces. A `0.000` gap is a true tie the depth test
//! cannot decide; anything else is a separation to set against the depth buffer's resolution at
//! that range. Output is Blizzard data: never commit it.
//! `cargo run -p benilla-formats --example wmo_coplanar -- <wmo-path-or-substring> [gap_yd]`

use benilla_formats::RenderSubmesh;

/// Faces this close to parallel are treated as the same plane orientation (≈2.6°).
const PARALLEL_DOT: f32 = 0.999;

/// One triangle as its plane and bounding box.
struct Face {
    normal: [f32; 3],
    /// Plane offset: `normal · vertex`.
    d: f32,
    min: [f32; 3],
    max: [f32; 3],
    /// The corners, so a batch can be compared with itself: two halves of one flat quad share a
    /// vertex, overlapping sheets do not.
    verts: [[f32; 3]; 3],
}

fn faces(sub: &RenderSubmesh) -> Vec<Face> {
    sub.indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter_map(|t| {
            let p: Vec<[f32; 3]> = t.iter().map(|&i| sub.positions[i as usize]).collect();
            let (u, v) = (sub3(p[1], p[0]), sub3(p[2], p[0]));
            let n = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len < 1e-9 {
                return None; // degenerate triangle: no plane to speak of
            }
            let normal = [n[0] / len, n[1] / len, n[2] / len];
            let mut min = p[0];
            let mut max = p[0];
            for q in &p[1..] {
                for c in 0..3 {
                    min[c] = min[c].min(q[c]);
                    max[c] = max[c].max(q[c]);
                }
            }
            Some(Face {
                d: dot(normal, p[0]),
                normal,
                min,
                max,
                verts: [p[0], p[1], p[2]],
            })
        })
        .collect()
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn boxes_touch(a: &Face, b: &Face, slack: f32) -> bool {
    (0..3).all(|c| a.min[c] - slack <= b.max[c] && b.min[c] - slack <= a.max[c])
}

fn shares_a_vertex(a: &Face, b: &Face) -> bool {
    a.verts.iter().any(|p| {
        b.verts
            .iter()
            .any(|q| (0..3).all(|c| (p[c] - q[c]).abs() < 1e-5))
    })
}

/// The smallest plane gap within `gap` between near-parallel overlapping faces of `a` and `b`;
/// within one batch, faces sharing a vertex are skipped.
fn closest_coplanar(a: &[Face], b: &[Face], gap: f32, same_batch: bool) -> Option<f32> {
    let mut best: Option<f32> = None;
    for (i, fa) in a.iter().enumerate() {
        // Within one batch every unordered pair is visited once, and a face never fights itself.
        let rest = if same_batch { &b[i + 1..] } else { b };
        for fb in rest {
            if !boxes_touch(fa, fb, gap) || (same_batch && shares_a_vertex(fa, fb)) {
                continue;
            }
            let d = dot(fa.normal, fb.normal);
            if d.abs() < PARALLEL_DOT {
                continue;
            }
            // Flip B's plane to A's orientation so the offsets are comparable.
            let db = if d < 0.0 { -fb.d } else { fb.d };
            let g = (fa.d - db).abs();
            if g <= gap && best.is_none_or(|x| g < x) {
                best = Some(g);
            }
        }
    }
    best
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let pat = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: wmo_coplanar <wmo-path-or-substring> [gap_yd]"))?;
    // Pairs within this gap; the default is wide enough to show near misses too.
    let gap: f32 = args.next().map_or(Ok(0.05), |g| g.parse())?;

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

    let subs = benilla_formats::load_wmo(&mut chain, &path)?;
    println!(
        "=== {} batches (spawn order = depth-bias order) ===",
        subs.len()
    );
    for (i, s) in subs.iter().enumerate() {
        let n = s.positions.len();
        let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &s.positions {
            for c in 0..3 {
                min[c] = min[c].min(p[c]);
                max[c] = max[c].max(p[c]);
            }
        }
        println!(
            "[{i:3}] bias {:3}  {:5} tri  {:?}{}{}  span ({:.1},{:.1},{:.1})..({:.1},{:.1},{:.1})  {}",
            i + 1,
            s.indices.len() / 3,
            s.blend,
            if s.interior { " INT" } else { "" },
            if s.emissive { " UNLIT" } else { "" },
            min[0], min[1], min[2], max[0], max[1], max[2],
            s.texture.as_deref().unwrap_or("(none)"),
        );
        let _ = n;
    }

    let all: Vec<Vec<Face>> = subs.iter().map(faces).collect();
    // Facing is ignored throughout: two coplanar faces z-fight whether or not their normals agree.
    println!("\n=== batches coplanar WITH THEMSELVES (gap <= {gap}) ===");
    println!("(no per-batch depth bias can separate these — the pipeline draws them as one call)");
    let mut self_found = 0;
    for (i, f) in all.iter().enumerate() {
        if let Some(g) = closest_coplanar(f, f, gap, true) {
            self_found += 1;
            println!(
                "[{i:3}] gap {g:.5} yd  {}",
                subs[i].texture.as_deref().unwrap_or("(none)")
            );
        }
    }
    if self_found == 0 {
        println!("(none)");
    }

    println!("\n=== coplanar batch pairs (gap <= {gap}) ===");
    let mut found = 0;
    for a in 0..all.len() {
        for b in (a + 1)..all.len() {
            if let Some(g) = closest_coplanar(&all[a], &all[b], gap, false) {
                found += 1;
                println!(
                    "[{a:3}] x [{b:3}]  gap {g:.5} yd  {}  |  {}",
                    subs[a].texture.as_deref().unwrap_or("(none)"),
                    subs[b].texture.as_deref().unwrap_or("(none)"),
                );
            }
        }
    }
    if found == 0 {
        println!("(none — no two batches share a plane within {gap} yd)");
    }
    Ok(())
}
