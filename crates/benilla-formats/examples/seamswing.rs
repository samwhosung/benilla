//! Where does a welded billboard bone's geometry SIT relative to its pivot — how far, and along
//! which local axis?
//!
//! Two readings, and the second is the one that decides anything. **Distance** answers the seam
//! question: a partially-weighted seam vertex is dragged by its weight times the re-orientation and
//! linear-blend skinning shrinks it toward the pivot on the way (the candy-wrapper), so a seam ring
//! sitting ON the pivot hinges cleanly and one sitting far out tears.
//!
//! **Direction answers whether the geometry moves at all.** A spherical billboard maps the bone's
//! local axes onto fixed camera axes, so an offset lying along a single local axis lands on the same
//! camera axis from every angle: it does not sweep, it only loses its roll and its foreshortening.
//! Only an offset spread across the axes sweeps. Magnitude alone cannot tell those apart, and
//! reading a 0.29 yd radius as a 0.29 yd arc is exactly how decision 0847 withdrew a correct change
//! — the spikes it measured run **along** their bone (worst vertex 12° off axis), so the sweep it
//! described never existed (the spherical basis `0x71547c`, and this tool's own output).
//!
//! Usage: `cargo run -p benilla-formats --example seamswing -- <WoW/Data> <internal\path.m2>`

use benilla_formats::open_chain;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    // An explicit dir still wins; with none, ask the one resolver (decision 1175).
    let data = args
        .next()
        .map(std::path::PathBuf::from)
        .or_else(benilla_formats::wow_data)
        .expect("no WoW install found (set $WOW_DATA or pass the dir)");
    let path = args.next().expect("usage: seamswing [data] <path.m2>");
    let mut chain = open_chain(&data)?;
    let bytes = chain.read_file(&path)?;
    let denied = benilla_formats::non_separable_billboard_bones(&bytes);
    println!("welded billboard bones: {denied:?}");

    let model = benilla_m2::parse_m2(&mut std::io::Cursor::new(&bytes))?;
    let model = model.model();
    for &b in &denied {
        let bone = &model.bones[b as usize];
        let piv = [bone.pivot.x, bone.pivot.y, bone.pivot.z];
        // Every vertex with any weight on this bone: its weight, and its distance from the pivot —
        // which is the radius the re-orientation sweeps it around.
        let (mut pure, mut seam) = (Vec::new(), Vec::new());
        for v in &model.vertices {
            for i in 0..4 {
                if v.bone_indices[i] as u16 != b || v.bone_weights[i] == 0 {
                    continue;
                }
                let off = [
                    v.position.x - piv[0],
                    v.position.y - piv[1],
                    v.position.z - piv[2],
                ];
                let d = (off[0].powi(2) + off[1].powi(2) + off[2].powi(2)).sqrt();
                let w = f32::from(v.bone_weights[i]) / 255.0;
                if w > 0.999 {
                    pure.push((d, off))
                } else {
                    seam.push((w, d))
                }
            }
        }
        let stat = |v: &[f32]| {
            if v.is_empty() {
                return "—".to_string();
            }
            let (mn, mx) = v
                .iter()
                .fold((f32::MAX, 0.0f32), |(a, b), &d| (a.min(d), b.max(d)));
            format!(
                "n={} min={mn:.4} max={mx:.4} mean={:.4}",
                v.len(),
                v.iter().sum::<f32>() / v.len() as f32
            )
        };
        let seam_d: Vec<f32> = seam.iter().map(|&(_, d)| d).collect();
        let pure_d: Vec<f32> = pure.iter().map(|&(d, _)| d).collect();
        println!(
            "bone {b} pivot ({:.3}, {:.3}, {:.3})",
            piv[0], piv[1], piv[2]
        );
        println!("  fully-weighted verts, |v−pivot|: {}", stat(&pure_d));
        println!("  SEAM verts,          |v−pivot|: {}", stat(&seam_d));
        if let Some(&(w, _)) = seam.first() {
            println!("  seam weight on this bone: {w:.2}");
        }
        // DIRECTION, which is what actually decides whether re-orientation moves the geometry.
        // A spherical billboard maps the bone's local axes to fixed camera axes, so an offset that
        // lies along ONE local axis lands on that camera axis from every angle: it does not sweep,
        // it only loses its roll and its foreshortening. An offset spread across the axes does
        // sweep. Magnitude alone cannot tell these apart — decision 0847 read a 0.29 yd radius as a
        // 0.29 yd arc and withdrew a correct change on the strength of it.
        if !pure.is_empty() {
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            for &(_, o) in &pure {
                for k in 0..3 {
                    lo[k] = lo[k].min(o[k]);
                    hi[k] = hi[k].max(o[k]);
                }
            }
            // The DIRECTION axis is the one the offsets are largest along — NOT the one they are
            // most spread across. Those differ here and confusing them inverts the reading: this
            // spike runs along z (|z| ≈ 0.29) but is widest across y (spread 0.105, its cross
            // section). Report both, named, so neither can be mistaken for the other.
            let mean_abs =
                |k: usize| pure.iter().map(|&(_, o)| o[k].abs()).sum::<f32>() / pure.len() as f32;
            let along = (0..3)
                .max_by(|&a, &c| mean_abs(a).total_cmp(&mean_abs(c)))
                .unwrap();
            let widest = (0..3)
                .max_by(|&a, &c| (hi[a] - lo[a]).total_cmp(&(hi[c] - lo[c])))
                .unwrap();
            // How far the WORST vertex strays from that one axis — the number that decides whether
            // "runs along a single axis" is a fair description of the whole batch or only its mean.
            let worst_deg = pure
                .iter()
                .filter(|&&(d, _)| d > 1e-6)
                .map(|&(d, o)| (o[along].abs() / d).clamp(-1.0, 1.0).acos().to_degrees())
                .fold(0.0f32, f32::max);
            println!(
                "  offset from pivot, per axis: x[{:+.3},{:+.3}] y[{:+.3},{:+.3}] z[{:+.3},{:+.3}]",
                lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]
            );
            println!(
                "  → RUNS ALONG {} (mean |{}| = {:.3}); widest across {} (cross section {:.3}); \
                 worst vertex {worst_deg:.1}° off {}",
                ["x", "y", "z"][along],
                ["x", "y", "z"][along],
                mean_abs(along),
                ["x", "y", "z"][widest],
                hi[widest] - lo[widest],
                ["x", "y", "z"][along],
            );
        }
    }
    Ok(())
}
