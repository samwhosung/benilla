//! Which faces of a WMO are **authored twice** — two triangles on the same three positions:
//! `cargo run -p benilla-formats --example wmo_doubled -- <wmo-path-or-substring>`
//! e.g. `wmo_doubled orctower`.
//!
//! The falsifier `wmo_coplanar` structurally cannot be. That tool skips same-batch face pairs that
//! share a vertex ("two halves of a quad") — but a thin sheet whose two *sides* are authored by
//! reusing the same vertices with **reversed winding** shares all three, so a doubled canvas awning
//! is invisible to it by construction (it reported the B38 tent's nearest partner at 1.91 yd while
//! the tent's own two sides sat at 0.000). This tool asks the opposite question: which triangles
//! occupy the SAME three positions?
//!
//! Doubled-with-opposite-winding is not a defect — it is how 1.12 authors visible-from-both-sides
//! cloth when the material is single-sided: the real client backface-culls each copy from the wrong
//! side, so exactly one covers any pixel. It *becomes* a defect the moment the renderer draws WMO
//! batches two-sided — ours did until `3af8854f` honoured MOMT `0x04`: both
//! copies then rasterize at depths equal to the ulp, and the winner per pixel is floating-point
//! noise — latched while the camera is bit-still, flickering while anything creeps. That is the
//! defect this tool was built to name, and it is now closed; it stays as the census. Each pair
//! prints with its
//! batches' MOMT flags: `0x04` (UNCULLED) set means the file *asked* for two-sided and the double
//! draw is authored intent; clear means the renderer's shortcut is manufacturing a z-tie the file
//! never contained.
//!
//! Output is Blizzard data — pipe it to the scratchpad, never into the repo.

use std::collections::HashMap;

use benilla_wmo::{parse_wmo, ParsedWmo};

/// Positions are matched to a tenth of a millimetre — far under authoring precision, far over f32
/// noise at building scale.
const QUANT: f32 = 10_000.0;

fn key(p: [f32; 3]) -> [i64; 3] {
    p.map(|c| (c * QUANT).round() as i64)
}

/// A triangle's identity (order-free) plus the data needed to classify its partner.
struct Tri {
    batch: usize,
    /// Geometric normal in original winding order — two copies with `dot < 0` are opposite-wound.
    normal: [f32; 3],
    /// Mean authored (MONR) normal, to name which side of the sheet this copy is.
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

    // The root's MOMT table, raw — the submeshes don't carry material flags, and the flags decide
    // whether a doubled pair is authored intent (0x04 UNCULLED) or a renderer-manufactured tie.
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

    // One global bucket per geometric triangle, across every batch: any bucket holding two entries
    // is a doubled face, wherever its copies live.
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

    // Doubled-face census per batch pair, split by winding — opposite is the two-sided-sheet
    // authoring; same-winding true duplicates would be a different (and stranger) finding.
    /// Per batch pair: doubled-face count + each side's summed authored normal.
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
                    // Accumulate each side's authored normal by batch order, so the report can say
                    // "batch A faces up, batch B faces down" rather than just "they differ".
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
