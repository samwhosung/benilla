//! **Is the eye emitter still proud of the face once the model ANIMATES?**
//!
//! `eye_quad_pass` answers the depth contest at *bind* pose, where benilla already matches the
//! reference. Decision 0707 measured the failing case and found the split: benilla's freshest eye
//! particle — born this frame at the current bone pose — sits **1.6–4.7 cm behind the face our
//! renderer draws**, against **2.5–34 mm** at bind. "Faithful at rest" and "faithful in motion" are
//! different claims, and this is the instrument for the second one.
//!
//! It builds the bone palette ourselves, from our own parse, at sampled times across a sequence:
//! `palette[b] = parentPalette · T(pivot) · R(rot) · S(scale) · T(-pivot)`, the M2 skinning matrix.
//! Then, per sample, it reports the two numbers that decide the contest:
//!
//! - `gap` — the distance from the animated eye-bone pivot (where our particles are born, verified
//!   `birth == joint`) to the **nearest skinned vertex** of the depth-writing batches. At bind this
//!   is 0.0162 for bone 60 (measured on the shipped `Voidwalker.m2`). If our animation swings the
//!   skin centimetres past the bone, this grows — and that growth *is* the bug.
//! - `pass%` — the constant-depth quad's surviving area against the animated shell, front view.
//!
//! `cargo run -p benilla-formats --example eye_burial_anim -- 'Creature\Voidwalker\Voidwalker.m2' 0`
//!
//! A `gap` that stays near its bind value across the cycle says our pose is self-consistent and 0707's
//! centimetres came from somewhere else in the renderer. A `gap` that blows out says our skinning or
//! our animation moved the face off the bone, and the fix is there.

use benilla_formats::{ModelAnimation, RenderSubmesh, Skeleton};
use glam::{Mat4, Quat, Vec3};

/// The M2 skinning palette at time `t` (seconds into `anim`), in file-bone order.
///
/// The pivot dance is the whole point: a bone's animated rotation/scale act **about its own pivot**,
/// so the matrix is `T(pivot) · R · S · T(-pivot)` composed under the parent. At rest (no keys) every
/// factor is identity and the palette is identity — which is why an emitter record `position` that
/// equals its bone pivot maps to itself at bind.
fn palette(skel: &Skeleton, anim: &ModelAnimation, t: f32) -> Vec<Mat4> {
    let sample_v = |keys: &[(f32, [f32; 3])], t: f32, dflt: Vec3| -> Vec3 {
        if keys.is_empty() {
            return dflt;
        }
        match keys.iter().position(|&(kt, _)| kt >= t) {
            None => Vec3::from(keys[keys.len() - 1].1),
            Some(0) => Vec3::from(keys[0].1),
            Some(i) => {
                let (t0, a) = keys[i - 1];
                let (t1, b) = keys[i];
                let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
                Vec3::from(a).lerp(Vec3::from(b), f)
            }
        }
    };
    let sample_q = |keys: &[(f32, [f32; 4])], t: f32| -> Quat {
        if keys.is_empty() {
            return Quat::IDENTITY;
        }
        let q = |v: [f32; 4]| Quat::from_xyzw(v[0], v[1], v[2], v[3]).normalize();
        match keys.iter().position(|&(kt, _)| kt >= t) {
            None => q(keys[keys.len() - 1].1),
            Some(0) => q(keys[0].1),
            Some(i) => {
                let (t0, a) = keys[i - 1];
                let (t1, b) = keys[i];
                let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
                q(a).slerp(q(b), f)
            }
        }
    };

    let mut out = vec![Mat4::IDENTITY; skel.bones.len()];
    for (i, bone) in skel.bones.iter().enumerate() {
        let keys = anim.bones.iter().find(|k| k.bone as usize == i);
        let (tr, rot, sc) = match keys {
            Some(k) => (
                sample_v(&k.translation, t, Vec3::ZERO),
                sample_q(&k.rotation, t),
                sample_v(&k.scale, t, Vec3::ONE),
            ),
            None => (Vec3::ZERO, Quat::IDENTITY, Vec3::ONE),
        };
        let pivot = Vec3::from(bone.pivot);
        let local = Mat4::from_translation(pivot + tr)
            * Mat4::from_quat(rot)
            * Mat4::from_scale(sc)
            * Mat4::from_translation(-pivot);
        out[i] = match bone.parent {
            p if p >= 0 && (p as usize) < i => out[p as usize] * local,
            _ => local,
        };
    }
    out
}

/// Skin one vertex by its 4 bone/weight pairs (weights already normalised by the parser).
fn skin(p: [f32; 3], j: [u16; 4], w: [f32; 4], pal: &[Mat4]) -> Vec3 {
    let v = Vec3::from(p);
    let mut acc = Vec3::ZERO;
    let mut total = 0.0;
    for k in 0..4 {
        if w[k] <= 0.0 {
            continue;
        }
        let Some(m) = pal.get(j[k] as usize) else {
            continue;
        };
        acc += m.transform_point3(v) * w[k];
        total += w[k];
    }
    if total > 0.0 {
        acc / total
    } else {
        v
    }
}

/// Every skinned triangle of the depth-writing batches, plus the nearest-vertex distance to `probe`.
fn shell(subs: &[RenderSubmesh], pal: &[Mat4], probe: Vec3) -> (Vec<[Vec3; 3]>, f32) {
    let mut tris = Vec::new();
    let mut near = f32::MAX;
    for s in subs {
        if s.no_depth_write || s.no_depth_test || s.joints.is_empty() {
            continue;
        }
        let pos: Vec<Vec3> = (0..s.positions.len())
            .map(|i| skin(s.positions[i], s.joints[i], s.weights[i], pal))
            .collect();
        for &p in &pos {
            near = near.min(p.distance(probe));
        }
        for t in s.indices.as_chunks::<3>().0 {
            tris.push([pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]]);
        }
    }
    (tris, near)
}

fn ray_hits(tris: &[[Vec3; 3]], orig: Vec3, dir: Vec3) -> bool {
    tris.iter().any(|t| {
        let (e1, e2) = (t[1] - t[0], t[2] - t[0]);
        let h = dir.cross(e2);
        let det = e1.dot(h);
        if det.abs() < 1e-9 {
            return false;
        }
        let inv = 1.0 / det;
        let s = orig - t[0];
        let u = inv * s.dot(h);
        if !(0.0..=1.0).contains(&u) {
            return false;
        }
        let q = s.cross(e1);
        let v = inv * dir.dot(q);
        if v < 0.0 || u + v > 1.0 {
            return false;
        }
        inv * e2.dot(q) > 1e-6
    })
}

/// Surviving area fraction of the constant-depth quad at `center`, viewed along `view`.
fn pass_fraction(tris: &[[Vec3; 3]], center: Vec3, half: f32, view: Vec3, n: usize) -> f32 {
    let up = if view.z.abs() > 0.9 { Vec3::X } else { Vec3::Z };
    let right = view.cross(up).normalize();
    let real_up = right.cross(view).normalize();
    let mut passed = 0usize;
    for iy in 0..n {
        for ix in 0..n {
            let fx = (ix as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let fy = (iy as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let p = center + right * (fx * half) + real_up * (fy * half);
            if !ray_hits(tris, p, -view) {
                passed += 1;
            }
        }
    }
    passed as f32 / (n * n) as f32
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let virt = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: eye_burial_anim <m2 path> [seq slot]"))?;
    let slot: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let bytes = chain.read_file(&virt)?;

    let subs = benilla_formats::parse_m2_render_submeshes(&bytes, "", &[])?;
    let skel = benilla_formats::parse_m2_skeleton(&bytes)?;
    let anims = benilla_formats::parse_m2_animations(&bytes);
    let anim = anims
        .iter()
        .find(|a| a.seq_index == slot)
        .ok_or_else(|| anyhow::anyhow!("no sequence in file slot {slot}"))?;
    let emitters = benilla_formats::parse_m2_particle_emitters(&bytes)?;

    println!(
        "{virt}  seq slot {slot} (anim id {}, {:.3}s, {} bones keyed)",
        anim.anim_id,
        anim.duration,
        anim.bones.len()
    );

    // Which bones does the depth-writing shell actually skin to, and does any of them sit under a
    // BILLBOARD bone? A billboard joint is re-aimed at the camera every frame and its children
    // inherit that rotation, so shell geometry under one moves with the *camera* — which is exactly
    // the signature of a burial that appears from one camera elevation and not another.
    {
        let mut shell_bones: Vec<usize> = Vec::new();
        for s in &subs {
            if s.no_depth_write || s.no_depth_test {
                continue;
            }
            for (j, w) in s.joints.iter().zip(&s.weights) {
                for k in 0..4 {
                    if w[k] > 0.0 && !shell_bones.contains(&(j[k] as usize)) {
                        shell_bones.push(j[k] as usize);
                    }
                }
            }
        }
        shell_bones.sort_unstable();
        let billboard: Vec<usize> = skel
            .bones
            .iter()
            .enumerate()
            .filter(|(_, b)| b.billboard.is_some())
            .map(|(i, _)| i)
            .collect();
        let mut tainted = Vec::new();
        for &b in &shell_bones {
            let (mut cur, mut hops) = (b, 0);
            while cur < skel.bones.len() && hops < 64 {
                if skel.bones[cur].billboard.is_some() {
                    tainted.push((b, cur));
                    break;
                }
                match skel.bones[cur].parent {
                    p if p >= 0 => cur = p as usize,
                    _ => break,
                }
                hops += 1;
            }
        }
        println!(
            "shell skins to {} bones {:?}\nbillboard bones: {billboard:?}\nshell bones under a billboard: {tainted:?}",
            shell_bones.len(),
            shell_bones
        );
    }

    for e in &emitters {
        let b = e.bone as usize;
        if b >= skel.bones.len() {
            continue;
        }
        // Only the additive quad emitters mounted flush in the shell are the B16 subject; the
        // others are printed anyway so the contrast is on the same page.
        println!(
            "\nemitter bone {b}  pos ({:.4},{:.4},{:.4})  blend {:?}",
            e.position[0], e.position[1], e.position[2], e.blend
        );
        println!("     t     gap(m)   vs bind    pass%(front)  pass%(30 above)");
        let mut bind_gap = f32::NAN;
        for step in 0..=8 {
            let t = anim.duration * step as f32 / 8.0;
            let pal = palette(&skel, anim, if step == 0 { 0.0 } else { t });
            // Where our particles are born: the emitter record position through the same palette.
            let center = pal[b].transform_point3(Vec3::from(e.position));
            let (tris, near) = shell(&subs, &pal, center);
            if step == 0 {
                bind_gap = near;
            }
            let front = pass_fraction(&tris, center, 0.0833, Vec3::new(-1.0, 0.0, 0.0), 48);
            let el = 30f32.to_radians();
            let above = pass_fraction(
                &tris,
                center,
                0.0833,
                Vec3::new(-el.cos(), 0.0, -el.sin()),
                48,
            );
            println!(
                "  {t:5.3}   {near:7.4}   {:+7.4}      {:5.1}          {:5.1}",
                near - bind_gap,
                front * 100.0,
                above * 100.0
            );
        }
    }
    Ok(())
}
