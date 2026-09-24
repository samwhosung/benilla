//! The share of a particle emitter's constant-depth quad that passes `LEQUAL` against its own
//! model's depth-writing batches, per camera direction, from our parse alone. The reference's quad
//! writer (`0x7b2a50`) gives 43.4% front, 38.8% 30° above and 43.3% 60° left for `Voidwalker.m2`
//! bone 60 at half-size 0.0833; a mismatch puts the bug in our asset side, a match in the renderer.
//! Model space, rest pose, WoW axes (X forward, Y left, Z up); the emitter `position`, equal to its
//! bone's pivot, is the quad centre. `cargo run -p benilla-formats --example eye_quad_pass -- <m2>`

use benilla_formats::RenderSubmesh;

/// Möller-Trumbore: the ray parameter `t` of a hit ahead of the origin.
fn ray_tri(orig: [f32; 3], dir: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Option<f32> {
    let sub = |p: [f32; 3], q: [f32; 3]| [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
    let cross = |p: [f32; 3], q: [f32; 3]| {
        [
            p[1] * q[2] - p[2] * q[1],
            p[2] * q[0] - p[0] * q[2],
            p[0] * q[1] - p[1] * q[0],
        ]
    };
    let dot = |p: [f32; 3], q: [f32; 3]| p[0] * q[0] + p[1] * q[1] + p[2] * q[2];

    let (e1, e2) = (sub(b, a), sub(c, a));
    let h = cross(dir, e2);
    let det = dot(e1, h);
    if det.abs() < 1e-9 {
        return None; // ray parallel to the triangle plane
    }
    // Two-sided on purpose: the depth buffer holds inward-facing triangles too.
    let inv = 1.0 / det;
    let s = sub(orig, a);
    let u = inv * dot(s, h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = inv * dot(dir, q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = inv * dot(e2, q);
    (t > 1e-6).then_some(t)
}

fn screen_axes(view: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    // `view` points from the camera toward the model; any up reference not parallel to it.
    let up = if view[2].abs() > 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let cross = |p: [f32; 3], q: [f32; 3]| {
        [
            p[1] * q[2] - p[2] * q[1],
            p[2] * q[0] - p[0] * q[2],
            p[0] * q[1] - p[1] * q[0],
        ]
    };
    let norm = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let right = norm(cross(view, up));
    let real_up = norm(cross(right, view));
    (right, real_up)
}

/// The share of the quad that passes `LEQUAL`: every sample sits at the quad's one depth, so it
/// fails iff a depth-writing triangle lies between it and the camera.
fn pass_fraction(
    tris: &[([f32; 3], [f32; 3], [f32; 3])],
    center: [f32; 3],
    half: f32,
    view: [f32; 3],
    n: usize,
) -> f32 {
    let (right, up) = screen_axes(view);
    // Cast toward the camera: anything hit is in front of the sample point.
    let back = [-view[0], -view[1], -view[2]];
    let mut passed = 0usize;
    let mut total = 0usize;
    for iy in 0..n {
        for ix in 0..n {
            let fx = (ix as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let fy = (iy as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let p = [
                center[0] + right[0] * fx * half + up[0] * fy * half,
                center[1] + right[1] * fx * half + up[1] * fy * half,
                center[2] + right[2] * fx * half + up[2] * fy * half,
            ];
            total += 1;
            if !tris
                .iter()
                .any(|&(a, b, c)| ray_tri(p, back, a, b, c).is_some())
            {
                passed += 1;
            }
        }
    }
    passed as f32 / total as f32
}

/// The occluder set in model space: every triangle of every depth-writing batch, or with `all` of
/// every batch, as benilla's fade blend twin writes depth regardless of `no_depth_write`.
fn occluders(subs: &[RenderSubmesh], all: bool) -> Vec<([f32; 3], [f32; 3], [f32; 3])> {
    let mut tris = Vec::new();
    for s in subs {
        if !all && (s.no_depth_write || s.no_depth_test) {
            continue;
        }
        for t in s.indices.as_chunks::<3>().0 {
            tris.push((
                s.positions[t[0] as usize],
                s.positions[t[1] as usize],
                s.positions[t[2] as usize],
            ));
        }
    }
    tris
}

fn main() -> anyhow::Result<()> {
    let virt = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: eye_quad_pass <m2 path>"))?;
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let bytes = chain.read_file(&virt)?;

    let subs = benilla_formats::parse_m2_render_submeshes(&bytes, "", &[])?;
    let tris = occluders(&subs, false);
    let tris_all = occluders(&subs, true);
    let writing: Vec<usize> = subs
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.no_depth_write && !s.no_depth_test)
        .map(|(i, _)| i)
        .collect();
    println!(
        "{} batches, {} z-writing ({:?}), {} occluding triangles",
        subs.len(),
        writing.len(),
        writing,
        tris.len()
    );

    // (label, azimuth° about +Z from +X, elevation°); the camera sits at -view.
    let views: &[(&str, f32, f32)] = &[
        ("front (+X)", 0.0, 0.0),
        ("15 above", 0.0, 15.0),
        ("30 above", 0.0, 30.0),
        ("45 above", 0.0, 45.0),
        ("60 above", 0.0, 60.0),
        ("30 left", 30.0, 0.0),
        ("60 left", 60.0, 0.0),
        ("30 left + 15 above", 30.0, 15.0),
        ("30 left + 30 above", 30.0, 30.0),
    ];

    for e in benilla_formats::parse_m2_particle_emitters(&bytes)?.iter() {
        // Meaningful only for a quad emitter mounted in the mesh; every emitter prints.
        println!(
            "\nemitter bone {} pos ({:.4}, {:.4}, {:.4})  blend {:?}",
            e.bone, e.position[0], e.position[1], e.position[2], e.blend
        );
        for (set, occ) in [("flagged", &tris), ("ALL-write", &tris_all)] {
            let half = 0.0833f32;
            print!("  {set:9} half {half:.4}:");
            for &(label, az, el) in views {
                let (a, l) = (az.to_radians(), el.to_radians());
                let view = [-(a.cos() * l.cos()), -(a.sin() * l.cos()), -(l.sin())];
                let f = pass_fraction(occ, e.position, half, view, 64);
                print!("  {label} {:.1}%", f * 100.0);
            }
            println!();
        }
    }
    Ok(())
}
