//! Which faces of a WMO are authored twice, two triangles on the same three positions, with each
//! pair's MOMT flags. Opposite winding is how 1.12 authors cloth seen from both sides on a
//! single-sided material: the reference culls each copy from its back, so one covers any pixel,
//! and a renderer that draws the batches two-sided makes the copies z-fight. MOMT `0x04`
//! (UNCULLED) set means the file asks for two-sided drawing. `wmo_coplanar` cannot see these, as it
//! skips same-batch faces that share a vertex. Output is Blizzard data: never commit it.
//! `cargo run -p benilla-formats --example wmo_doubled -- <wmo-path-or-substring>`

use std::collections::HashMap;

use benilla_wmo::{parse_wmo, ParsedWmo};

/// Positions match to a tenth of a millimetre, under authoring precision and over f32 noise.
const QUANT: f32 = 10_000.0;

fn key(p: [f32; 3]) -> [i64; 3] {
    p.map(|c| (c * QUANT).round() as i64)
}

/// A triangle's batch and facing, to classify its partner.
struct Tri {
    batch: usize,
    /// Geometric normal in authored winding; two copies with `dot < 0` are opposite-wound.
    normal: [f32; 3],
    /// Summed authored (MONR) normals, naming which side of the sheet this copy is.
    authored: [f32; 3],
}

fn main() -> anyhow::Result<()> {
    let pat = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: wmo_doubled <wmo-path-or-substring>"))?;
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

    // The root's raw MOMT table: the submeshes carry no material flags.
    let root_bytes = chain.read_file(&path.to_ascii_lowercase())?;
    let (materials, textures, tex_map) = match parse_wmo(&mut std::io::Cursor::new(&root_bytes))? {
        ParsedWmo::Root(r) => (r.materials, r.textures, r.texture_offset_index_map),
        ParsedWmo::Group(_) => anyhow::bail!("{path} is a group file, want the root"),
    };
    println!("=== MOMT ({} materials) ===", materials.len());
    for (i, m) in materials.iter().enumerate() {
        let tex = textures
            .get(m.get_texture1_index(&tex_map) as usize)
            .map(String::as_str)
            .unwrap_or("(none)");
        println!(
            "[{i:3}] flags {:#06x}{}  blend {}  {tex}",
            m.flags,
            if m.flags & 0x04 != 0 { " UNCULLED" } else { "" },
            m.blend_mode,
        );
    }

    let subs = benilla_formats::load_wmo(&mut chain, &path)?;

    // One bucket per geometric triangle across every batch; two entries make a doubled face.
    let mut buckets: HashMap<[[i64; 3]; 3], Vec<Tri>> = HashMap::new();
    for (bi, s) in subs.iter().enumerate() {
        for t in s.indices.as_chunks::<3>().0 {
            let p: Vec<[f32; 3]> = t.iter().map(|&i| s.positions[i as usize]).collect();
            let (u, v) = (sub3(p[1], p[0]), sub3(p[2], p[0]));
            let n = cross(u, v);
            if len(n) < 1e-9 {
                continue; // degenerate: no facing to double
            }
            let mut authored = [0.0f32; 3];
            for &i in t {
                let a = s.normals.get(i as usize).copied().unwrap_or([0.0; 3]);
                for c in 0..3 {
                    authored[c] += a[c];
                }
            }
            let mut k = [key(p[0]), key(p[1]), key(p[2])];
            k.sort_unstable();
            buckets.entry(k).or_default().push(Tri {
                batch: bi,
                normal: n,
                authored,
            });
        }
    }

    // Per batch pair, split by winding: opposite is a two-sided sheet, same is a true duplicate.
    /// Per batch pair: the doubled-face count and each side's summed authored normal.
    type OppositeCensus = HashMap<(usize, usize), (usize, [f32; 3], [f32; 3])>;
    let mut opposite: OppositeCensus = HashMap::new();
    let mut same: HashMap<(usize, usize), usize> = HashMap::new();
    for tris in buckets.values() {
        for i in 0..tris.len() {
            for j in (i + 1)..tris.len() {
                let (a, b) = (&tris[i], &tris[j]);
                let pair = (a.batch.min(b.batch), a.batch.max(b.batch));
                if dot(a.normal, b.normal) < 0.0 {
                    let e = opposite.entry(pair).or_insert((0, [0.0; 3], [0.0; 3]));
                    e.0 += 1;
                    // Each side's authored normal in batch order, to say which way each faces.
                    let (first, second) = if a.batch <= b.batch { (a, b) } else { (b, a) };
                    for c in 0..3 {
                        e.1[c] += first.authored[c];
                        e.2[c] += second.authored[c];
                    }
                } else {
                    *same.entry(pair).or_default() += 1;
                }
            }
        }
    }

    let describe = |bi: usize| {
        let s = &subs[bi];
        format!(
            "[{bi:3}] bias {:3} {:?} {}",
            bi + 1,
            s.blend,
            s.texture.as_deref().unwrap_or("(none)"),
        )
    };
    println!("\n=== doubled faces, OPPOSITE winding (the two-sided-sheet authoring) ===");
    let mut pairs: Vec<_> = opposite.iter().collect();
    pairs.sort_by_key(|(k, _)| **k);
    if pairs.is_empty() {
        println!("(none)");
    }
    for (&(a, b), &(count, na, nb)) in pairs {
        println!(
            "{count:5} tri  {}  <->  {}",
            describe(a),
            if a == b {
                "ITSELF".to_string()
            } else {
                describe(b)
            }
        );
        println!(
            "           side A authored normal ~[{:+.2},{:+.2},{:+.2}]   side B ~[{:+.2},{:+.2},{:+.2}]",
            na[0] / count as f32 / 3.0,
            na[1] / count as f32 / 3.0,
            na[2] / count as f32 / 3.0,
            nb[0] / count as f32 / 3.0,
            nb[1] / count as f32 / 3.0,
            nb[2] / count as f32 / 3.0,
        );
    }

    println!(
        "\n=== doubled faces, SAME winding (true duplicates — would double-draw even culled) ==="
    );
    if same.is_empty() {
        println!("(none)");
    }
    let mut pairs: Vec<_> = same.iter().collect();
    pairs.sort_by_key(|(k, _)| **k);
    for (&(a, b), &count) in pairs {
        println!(
            "{count:5} tri  {}  <->  {}",
            describe(a),
            if a == b {
                "ITSELF".to_string()
            } else {
                describe(b)
            }
        );
    }
    Ok(())
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(u: [f32; 3], v: [f32; 3]) -> [f32; 3] {
    [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn len(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}
