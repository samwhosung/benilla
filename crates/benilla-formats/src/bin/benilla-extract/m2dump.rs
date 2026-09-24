//! The single-model M2 dumps: each reads one `.m2` and prints all it carries for one concern.

use anyhow::{Context, Result};
use benilla_formats::{Chain, M2AnimSummary};

use crate::{normalize, yn};

/// Dump an M2's collision hull: counts, model-space AABB (WoW axes, Z up), extents.
pub fn m2coll(chain: &mut Chain, internal_path: &str) -> Result<()> {
    let name = normalize(internal_path);
    let hull = benilla_formats::load_m2_collision_hull(chain, &name)?;
    if hull.is_empty() {
        println!("no collision hull (nBoundingTriangles == 0) — nothing collides with this model");
        return Ok(());
    }
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &hull.positions {
        for a in 0..3 {
            min[a] = min[a].min(p[a]);
            max[a] = max[a].max(p[a]);
        }
    }
    println!(
        "{} vertices, {} triangles",
        hull.positions.len(),
        hull.triangle_count()
    );
    println!("aabb (model space, yd; WoW axes, Z up — placement scale multiplies):");
    for (axis, a) in ["x", "y", "z"].iter().zip(0..3) {
        println!(
            "  {axis}: {:>8.3} .. {:>8.3}   extent {:>7.3}",
            min[a],
            max[a],
            max[a] - min[a]
        );
    }
    Ok(())
}

/// Dump every sequence's event keyframes. On an attack clip, whether `$CPP` (the victim's defense)
/// precedes `$AH0-3`/`$CAH` (the impact) picks which exclusive reaction the swing record feeds.
pub fn m2events(chain: &mut Chain, internal_path: &str) -> Result<()> {
    let name = normalize(internal_path);
    let data = chain
        .read_file(&name)
        .with_context(|| format!("reading '{name}' from chain"))?;
    let seqs = benilla_formats::parse_m2_animations(&data);
    for (i, s) in seqs.iter().enumerate() {
        if s.events.is_empty() {
            continue;
        }
        let tags: Vec<String> = s
            .events
            .iter()
            .map(|e| {
                let ident = String::from_utf8_lossy(&e.ident);
                if e.data != 0 {
                    format!("{:.3}s {ident}({})", e.time, e.data)
                } else {
                    format!("{:.3}s {ident}", e.time)
                }
            })
            .collect();
        println!(
            "{i:>3}  anim {:>4}  dur {:>6.3}s  {}",
            s.anim_id,
            s.duration,
            tags.join("  ")
        );
    }
    Ok(())
}

/// Dump an M2's animation sequences in file order.
pub fn m2seq(chain: &mut Chain, internal_path: &str) -> Result<()> {
    let name = normalize(internal_path);
    let data = chain
        .read_file(&name)
        .with_context(|| format!("reading '{name}' from chain"))?;
    let seqs = benilla_formats::parse_m2_animations(&data);
    // `band` is the window on the keyframe timeline every non-global-sequence track keys in. `idx`
    // is not the file slot: zero-duration sequences are dropped.
    println!(
        "idx  anim   mode   dur(s)   mspd  blend   band(ms)          freq  replay   bones   keys"
    );
    for (i, s) in seqs.iter().enumerate() {
        // Bones with any keyed track in this band, and total T/R/S keys, clamp constants included.
        let bones = s
            .bones
            .iter()
            .filter(|b| !b.translation.is_empty() || !b.rotation.is_empty() || !b.scale.is_empty())
            .count();
        let keys: usize = s
            .bones
            .iter()
            .map(|b| b.translation.len() + b.rotation.len() + b.scale.len())
            .sum();
        println!(
            "{i:>3}  {:>4}  {}  {:>7.3}  {:>5.2}  {:>5.3}  {:>7}..{:<7}  {:>5}  ({}, {})  {bones:>5}  {keys:>5}",
            s.anim_id,
            if s.looping { "loop " } else { "clamp" },
            s.duration,
            // `mspd`: the authored move speed (yd/s) dividing the locomotion rate,
            // `speed / (mspd · |modelScale|)` (`0x5fe2f0` at 0x5fe4be..0x5fe550); 0 plays at 1×.
            s.move_speed,
            // `blend`: the blend-in time (s) the client cross-fades over on a blended arm (op4
            // `blendFlag != 0`, the `+0x98 -> +0xc4` snapshot decayed over `1/blendTime`); 0 cuts.
            s.blend_time,
            s.start_ms,
            s.end_ms,
            s.frequency,
            s.min_replay,
            s.max_replay,
        );
    }
    eprintln!("{} sequences", seqs.len());
    Ok(())
}

/// Dump an M2's attachment points (id + bone).
pub fn m2attach(chain: &mut Chain, internal_path: &str) -> Result<()> {
    let name = normalize(internal_path);
    let data = chain
        .read_file(&name)
        .with_context(|| format!("reading '{name}' from chain"))?;
    let attachments = benilla_formats::parse_m2_attachments(&data)?;
    // Raw WoW model space (X forward, Y left, Z up); item glows hang on ids 0..4 along a weapon.
    println!("id  bone  position (WoW model space)");
    for a in &attachments {
        let [x, y, z] = a.position;
        println!("{:>2}  {:>4}  [{x:.3} {y:.3} {z:.3}]", a.id, a.bone);
    }
    eprintln!("{} attachment points", attachments.len());
    Ok(())
}

fn print_txfm_track<V: std::fmt::Debug + Copy + PartialEq>(name: &str, t: &benilla_m2::M2Track<V>) {
    if t.keys.is_empty() {
        println!("    {name}: -");
        return;
    }
    let (t0, v0) = &t.keys[0];
    let (tn, vn) = &t.keys[t.keys.len() - 1];
    println!(
        "    {name}: {} key(s), interp {}, gseq {}, constant {}  [{t0} ms {v0:?} … {tn} ms {vn:?}]",
        t.keys.len(),
        t.interp,
        if t.gseq == 0xffff {
            "-".to_string()
        } else {
            t.gseq.to_string()
        },
        t.constant().is_some(),
    );
}

fn print_m2anim_summary(s: &M2AnimSummary, bytes: &[u8]) {
    println!("sequences: {}", s.sequence_count);
    println!(
        "  seq0 bone motion: {}  ({} bone(s) with a >1-key track)",
        if s.seq0_has_bone_motion { "yes" } else { "no" },
        s.seq0_animated_bone_count
    );
    println!(
        "global-sequence bone channels: {}",
        s.global_seq_channels.len()
    );
    for (bone, kind, period_ms) in &s.global_seq_channels {
        println!("  bone {bone:>3}  {kind}  period {period_ms} ms");
    }
    println!(
        "transparency tracks: {} total, {} animated (>1 key)",
        s.transparency_tracks.0, s.transparency_tracks.1
    );
    println!(
        "color rgb tracks:    {} total, {} animated (>1 key)",
        s.color_rgb_tracks.0, s.color_rgb_tracks.1
    );
    println!(
        "color alpha tracks:  {} total, {} animated (>1 key)",
        s.color_alpha_tracks.0, s.color_alpha_tracks.1
    );
    println!(
        "texture transforms: {} (header count; tracks unparsed)",
        s.texture_transform_count
    );
    println!("particle emitters:  {}", s.particle_emitter_count);
    let defs = benilla_formats::parse_m2_particle_emitters(bytes).unwrap_or_default();
    for (i, e) in s.emitter_bones.iter().enumerate() {
        println!(
            "  emitter {i}  bone {:>3}  flags {:#010x}  chain animates: {}",
            e.bone,
            e.flags,
            match (e.chain_seq0, e.chain_gseq) {
                (true, true) => "seq0 + gseq",
                (true, false) => "seq0",
                (false, true) => "gseq",
                (false, false) => "no (rest pose)",
            }
        );
        let Some(d) = defs.get(i) else { continue };
        // A burst emitter fires one `ftol(rate)` puff at its rate edge and never pours
        // (`0x718ec8` → `0x7b5550`).
        let burst = if d.burst() { "BURST " } else { "" };
        let views = d.timing.slot_views();
        let rate = match d.timing.constant_rate() {
            Some(r) => format!("{burst}rate {r:.1}/s"),
            None => {
                let per: Vec<String> = views
                    .iter()
                    .enumerate()
                    .map(|(s, (_, rate, _))| match rate {
                        Some(keys) => format!(
                            "s{s} {:?}",
                            keys.iter().map(|&(t, v)| (t, v as i32)).collect::<Vec<_>>()
                        ),
                        None => format!("s{s} -"),
                    })
                    .collect();
                format!("{burst}rate/seq [{}]", per.join("  "))
            }
        };
        // The enabled gate per sequence slot, in seconds from the slot's band start.
        let rate = if views.iter().all(|(_, _, e)| e.is_none()) {
            rate // no gate authored anywhere, the common case
        } else {
            let per: Vec<String> = views
                .iter()
                .enumerate()
                .map(|(s, (looping, _, enabled))| {
                    let clock = if *looping { "" } else { "!" };
                    match enabled {
                        Some(keys) => {
                            let w: Vec<String> = keys
                                .iter()
                                .map(|&(t, v)| {
                                    format!("{t:.2}:{}", if v > 0.5 { "on" } else { "off" })
                                })
                                .collect();
                            format!("s{s}{clock} {}", w.join(" "))
                        }
                        None => format!("s{s}{clock} on"),
                    }
                })
                .collect();
            format!("{rate}  enabled/seq [{}]", per.join("  "))
        };
        // A tail's streak length is |velocity| · tail_time.
        let tail = if d.head_tail >= 1 {
            format!(
                "  tail {:.2}s{}",
                d.tail_time,
                if d.tail_clamps_to_age() {
                    " (age-clamped)"
                } else {
                    ""
                }
            )
        } else {
            String::new()
        };
        // Each parameter channel's opening value; one that moves prints its keyed ramp below.
        let now = d.params.sample(None, 0.0, 0.0);
        println!(
            "             {:?} {:?} {}  {rate}  life {:.2}s  speed {:.2}  grav {:.2}  drag {:.1}{tail}  twinkle [{:.2}..{:.2}] spd {:.1} pct {:.2}  spin {:.2}",
            d.shape,
            d.blend,
            match d.head_tail {
                0 => "head",
                1 => "tail",
                _ => "head+tail",
            },
            now.lifespan,
            now.emission_speed,
            now.gravity,
            d.drag,
            d.twinkle_min,
            d.twinkle_max,
            d.twinkle_speed,
            d.twinkle_percent,
            d.spin,
        );
        for (name, slots) in d.params.channel_views() {
            for (s, keys) in slots.iter().enumerate() {
                if let Some(keys) = keys.filter(|k| k.len() > 1) {
                    let w: Vec<String> = keys
                        .iter()
                        .map(|&(t, v)| format!("({t:.3}s, {v:.3})"))
                        .collect();
                    println!("             ANIMATED {name}/s{s}: [{}]", w.join(", "));
                }
            }
        }
        // The kernel spread (`0x7b8d70`, `0x7b8890`): a sphere's ranges are latitude and longitude
        // about +X, its area the shell radii; a plane's a cone about +Z, its area the rectangle.
        let spread = match d.shape {
            benilla_formats::ParticleShape::Sphere => format!(
                "radius [{:.2}..{:.2}] lat ±{:.2} lon ±{:.2}",
                now.area_length, now.area_width, now.vertical_range, now.horizontal_range
            ),
            // A spline repurposes them (loader `0x70fa5e`-`0x70fae5`): area is tMin/tMax, vRange
            // the tangent spin, hRange the scatter.
            benilla_formats::ParticleShape::Spline => match &d.spline {
                Some(s) => format!(
                    "spline {} pts [{:.2} {:.2} {:.2} ..], t [{:.2}..{:.2}] spin ±{:.2} scatter {:.2}",
                    s.points.len(),
                    s.points[0][0],
                    s.points[0][1],
                    s.points[0][2],
                    now.area_length,
                    now.area_width,
                    now.vertical_range,
                    now.horizontal_range
                ),
                None => "spline UNPARSED (degenerate record)".to_string(),
            },
            _ => format!(
                "area {:.1}x{:.1} cone ±{:.2}/±{:.2}",
                now.area_length, now.area_width, now.vertical_range, now.horizontal_range
            ),
        };
        let zsrc = if now.z_source != 0.0 {
            format!("  zSource {:.2}", now.z_source)
        } else {
            String::new()
        };
        if let Some(g) = &d.geometry_model {
            println!("             MODEL-PARTICLES: {g}");
        }
        if let Some(r) = &d.recursion_model {
            println!("             CHILD-EMITTERS: {r}");
        }
        // The emitter-motion terms (`0x7b5230`): the follow-delta response's (speed, fraction)
        // samples and the velocity-inherit scale.
        let motion = match (d.follow_emitter(), d.inherits_emitter_motion()) {
            (false, false) => String::new(),
            (f, i) => {
                let mut s = String::new();
                if f {
                    s += &format!(
                        "  follow ({:.2}->{:.2}, {:.2}->{:.2})",
                        d.follow_speed1, d.follow_scale1, d.follow_speed2, d.follow_scale2
                    );
                }
                if i {
                    s += &format!("  inheritScale {:.2}", d.inherit_scale);
                }
                s
            }
        };
        println!(
            "             pos [{:.2} {:.2} {:.2}]  {spread}{zsrc}{motion}  texture: {}  cells {}x{}",
            d.position[0],
            d.position[1],
            d.position[2],
            d.texture.as_deref().unwrap_or("NONE (unresolved)"),
            d.tile_rows,
            d.tile_cols,
        );
        let c = d.over_life.color;
        println!(
            "             color/alpha keys: [{:.2} {:.2} {:.2} a{:.2}] -> [{:.2} {:.2} {:.2} a{:.2}] -> [{:.2} {:.2} {:.2} a{:.2}]  size {:?}",
            c[0][0], c[0][1], c[0][2], c[0][3], c[1][0], c[1][1], c[1][2], c[1][3], c[2][0],
            c[2][1], c[2][2], c[2][3], d.over_life.scale,
        );
    }
    println!("ribbon emitters:    {}", s.ribbon_emitter_count);
    // A keyed ribbon track prints its whole `(ms, value)` ramp; its first value alone can read 0.
    let scalar = |t: &benilla_formats::ValueTrack| -> String {
        match t.keys.len() {
            0 | 1 => format!("{:.2}", t.first()),
            _ => {
                let keys: Vec<String> = t
                    .keys
                    .iter()
                    .map(|&(ms, v)| format!("({ms}, {v:.2})"))
                    .collect();
                format!("keys [{}]", keys.join(", "))
            }
        }
    };
    for (i, r) in benilla_formats::parse_m2_ribbon_emitters(bytes)
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let rgb = if r.color.keys.len() <= 1 {
            let c = r.color.first();
            format!("[{:.2} {:.2} {:.2}]", c[0], c[1], c[2])
        } else {
            let keys: Vec<String> = r
                .color
                .keys
                .iter()
                .map(|&(ms, c)| format!("({ms}, [{:.2} {:.2} {:.2}])", c[0], c[1], c[2]))
                .collect();
            format!("keys [{}]", keys.join(", "))
        };
        println!(
            "  ribbon {i}  bone {:>3}  {:?}  {:.1} edges/s  life {:.2}s  g {:.2}  tex {}",
            r.bone,
            r.blend,
            r.edges_per_second,
            r.edge_lifetime,
            r.gravity,
            r.texture.as_deref().unwrap_or("NONE (unresolved)"),
        );
        println!(
            "            h above {}  below {}  rgb {}  a {}",
            scalar(&r.height_above),
            scalar(&r.height_below),
            rgb,
            scalar(&r.alpha),
        );
    }
    println!(
        "fully static: {}",
        if s.is_fully_static() { "yes" } else { "no" }
    );
}

/// Dump an M2's animation-channel summary plus texture-transform detail.
pub fn m2anim(chain: &mut Chain, internal_path: &str) -> Result<()> {
    let name = normalize(internal_path);
    let data = chain
        .read_file(&name)
        .with_context(|| format!("reading '{name}' from chain"))?;
    let summary = benilla_formats::parse_m2_animation_summary(&data)
        .with_context(|| format!("parsing M2 animation summary '{name}'"))?;
    print_m2anim_summary(&summary, &data);

    let fmt = benilla_m2::parse_m2(&mut std::io::Cursor::new(&data[..]))
        .with_context(|| format!("parsing M2 '{name}'"))?;
    let m = fmt.model();
    if !m.texture_transforms.is_empty() {
        println!("=== texture transforms ===");
        println!("lookup (header 0xac): {:?}", m.texture_transform_lookup);
        for (i, t) in m.texture_transforms.iter().enumerate() {
            println!("  transform {i}:");
            print_txfm_track("translation", &t.translation);
            print_txfm_track("rotation   ", &t.rotation);
            print_txfm_track("scaling    ", &t.scaling);
        }
        if let Ok(skin) = m.parse_embedded_skin(&data, 0) {
            for (bi, batch) in skin.batches().iter().enumerate() {
                println!(
                    "  batch {bi}: txfm combo {} (texture combo {}, material {})",
                    batch.texture_transform_combo_index,
                    batch.texture_combo_index,
                    batch.material_index
                );
            }
        }
    }
    if !m.color_alpha_tracks.is_empty() {
        println!("=== color alpha tracks (per M2Color) ===");
        for (i, t) in m.color_alpha_tracks.iter().enumerate() {
            println!("  color {i}: interp {}, keys {:?}", t.interp, t.keys);
        }
    }
    // The M2Colors' RGB half: the per-batch tint the client multiplies into the vertex colour.
    if m.color_rgb_tracks.iter().any(|t| t.keys.len() > 1) {
        println!("=== color rgb tracks (per M2Color) ===");
        for (i, t) in m.color_rgb_tracks.iter().enumerate() {
            let keys: Vec<String> = t
                .keys
                .iter()
                .map(|(ms, v)| format!("{ms} ms [{:.3} {:.3} {:.3}]", v[0], v[1], v[2]))
                .collect();
            println!(
                "  color {i}: interp {}, keys [{}]",
                t.interp,
                keys.join(", ")
            );
        }
    }
    if !m.transparency_tracks.is_empty() {
        println!("=== transparency (texture-weight) tracks ===");
        for (i, t) in m.transparency_tracks.iter().enumerate() {
            println!("  weight {i}: interp {}, keys {:?}", t.interp, t.keys);
        }
    }
    let seqs = benilla_formats::parse_m2_animations(&data);
    let any_scaled = seqs
        .iter()
        .any(|s| s.bones.iter().any(|b| !b.scale.is_empty()));
    if any_scaled {
        println!("=== bone scale tracks (per sequence; scale-keyed bones only) ===");
        for (si, s) in seqs.iter().enumerate() {
            for (bi, b) in s.bones.iter().enumerate() {
                if b.scale.is_empty() {
                    continue;
                }
                let keys: Vec<String> = b
                    .scale
                    .iter()
                    .map(|(t, v)| format!("{t:.3}s [{:.3} {:.3} {:.3}]", v[0], v[1], v[2]))
                    .collect();
                println!("  seq {si} bone {bi}: {}", keys.join(" -> "));
            }
        }
    }
    Ok(())
}

/// Dump an M2's bone table: KeyBoneID, flags, parent, pivot, and which sequences key each bone.
pub fn m2bones(chain: &mut Chain, internal_path: &str) -> Result<()> {
    let name = normalize(internal_path);
    let data = chain
        .read_file(&name)
        .with_context(|| format!("reading '{name}' from chain"))?;
    let fmt = benilla_m2::parse_m2(&mut std::io::Cursor::new(&data[..]))
        .with_context(|| format!("parsing M2 '{name}'"))?;
    let m = fmt.model();
    let seqs = benilla_formats::parse_m2_animations(&data);
    // `ign` spells out `flags & 0x7`: which of its parent's translation, scale and rotation the
    // bone refuses, taking the model root's instead.
    println!(
        "idx  keybone  flags       bb  ign  parent  pivot                       \
         keyed (seq[idx] T/R/S counts)"
    );
    for (i, b) in m.bones.iter().enumerate() {
        let ignore: String = match b.flags.bits() & 0x7 {
            0 => "-".into(),
            bits => ["T", "S", "R"]
                .iter()
                .enumerate()
                .filter(|(k, _)| bits & (1 << k) != 0)
                .map(|(_, n)| *n)
                .collect(),
        };
        let keyed: Vec<String> = seqs
            .iter()
            .enumerate()
            .filter_map(|(si, s)| {
                let bk = s.bones.iter().find(|bk| bk.bone as usize == i)?;
                Some(format!(
                    "seq{si}[T{} R{} S{}]",
                    bk.translation.len(),
                    bk.rotation.len(),
                    bk.scale.len()
                ))
            })
            .collect();
        println!(
            "{i:>3}  {:>7}  {:#010x}  {:>2}  {ignore:>3}  {:>6}  ({:>7.3}, {:>7.3}, {:>7.3})  {}",
            b.key_bone,
            b.flags.bits(),
            yn(b.is_billboard()),
            b.parent,
            b.pivot.x,
            b.pivot.y,
            b.pivot.z,
            keyed.join(" "),
        );
        // Small tracks print their key values: equal key counts can hide different values.
        for (si, s) in seqs.iter().enumerate() {
            let Some(bk) = s.bones.iter().find(|bk| bk.bone as usize == i) else {
                continue;
            };
            if !bk.translation.is_empty() && bk.translation.len() <= 8 {
                let keys: Vec<String> = bk
                    .translation
                    .iter()
                    .map(|(t, v)| format!("{t:.3}s ({:.3}, {:.3}, {:.3})", v[0], v[1], v[2]))
                    .collect();
                println!("       seq{si} (anim {}) T: {}", s.anim_id, keys.join("  "));
            }
            // Rotation keys as axis-angle (WoW model axes, Z up). A long track prints its first and
            // last keys, `step` (the first increment, whose axis is the spin axis) and `swing` (the
            // largest angle any key makes with the first, the amplitude `step` is not).
            if !bk.rotation.is_empty() {
                let aa = |q: &[f32; 4]| {
                    let w = q[3].clamp(-1.0, 1.0);
                    let angle = 2.0 * w.acos();
                    let s = (1.0 - w * w).sqrt();
                    let (x, y, z) = if s < 1e-5 {
                        (0.0, 0.0, 1.0)
                    } else {
                        (q[0] / s, q[1] / s, q[2] / s)
                    };
                    format!("{:+.2}°@({x:+.2},{y:+.2},{z:+.2})", angle.to_degrees())
                };
                if bk.rotation.len() <= 8 {
                    let keys: Vec<String> = bk
                        .rotation
                        .iter()
                        .map(|(t, q)| format!("{t:.3}s {}", aa(q)))
                        .collect();
                    println!("       seq{si} (anim {}) R: {}", s.anim_id, keys.join("  "));
                } else {
                    let (t0, q0) = &bk.rotation[0];
                    let (_t1, q1) = &bk.rotation[1];
                    let (tn, qn) = bk.rotation.last().unwrap();
                    // increment = q1 · q0⁻¹, whose axis is the spin axis.
                    let inv0 = [-q0[0], -q0[1], -q0[2], q0[3]];
                    let inc = [
                        q1[3] * inv0[0] + q1[0] * inv0[3] + q1[1] * inv0[2] - q1[2] * inv0[1],
                        q1[3] * inv0[1] - q1[0] * inv0[2] + q1[1] * inv0[3] + q1[2] * inv0[0],
                        q1[3] * inv0[2] + q1[0] * inv0[1] - q1[1] * inv0[0] + q1[2] * inv0[3],
                        q1[3] * inv0[3] - q1[0] * inv0[0] - q1[1] * inv0[1] - q1[2] * inv0[2],
                    ];
                    // `2·acos|⟨qi,q0⟩|` is the angle between two unit quats, sign-folded so a
                    // double-cover flip does not read as a 360° swing.
                    let swing = bk
                        .rotation
                        .iter()
                        .map(|(_, q)| {
                            let dot: f32 = (0..4).map(|k| q[k] * q0[k]).sum();
                            2.0 * dot.abs().clamp(0.0, 1.0).acos().to_degrees()
                        })
                        .fold(0.0f32, f32::max);
                    println!(
                        "       seq{si} (anim {}) R: {} keys  first {t0:.3}s {}  step {}  swing {swing:.2}°  last {tn:.3}s {}",
                        s.anim_id,
                        bk.rotation.len(),
                        aa(q0),
                        aa(&inc),
                        aa(qn),
                    );
                }
            }
            if !bk.scale.is_empty() && bk.scale.len() <= 8 {
                let keys: Vec<String> = bk
                    .scale
                    .iter()
                    .map(|(t, v)| format!("{t:.3}s ({:.3}, {:.3}, {:.3})", v[0], v[1], v[2]))
                    .collect();
                println!("       seq{si} (anim {}) S: {}", s.anim_id, keys.join("  "));
            }
        }
    }
    eprintln!("{} bones", m.bones.len());
    Ok(())
}

/// One batch's facing: its first triangle's winding normal, its mean vertex normal and their dot.
fn winding(s: &benilla_formats::RenderSubmesh) -> Option<String> {
    let tri = s.indices.get(..3)?;
    let p = |i: u32| s.positions.get(i as usize).copied();
    let (a, b, c) = (p(tri[0])?, p(tri[1])?, p(tri[2])?);
    let sub = |u: [f32; 3], v: [f32; 3]| [u[0] - v[0], u[1] - v[1], u[2] - v[2]];
    let cross = |u: [f32; 3], v: [f32; 3]| {
        [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ]
    };
    let norm = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        // A zero-area triangle has no direction; say so rather than print NaNs that read as data.
        (l > 1e-9).then(|| [v[0] / l, v[1] / l, v[2] / l])
    };
    let facet = norm(cross(sub(b, a), sub(c, a)));
    let vsum = s.normals.iter().fold([0.0f32; 3], |acc, n| {
        [acc[0] + n[0], acc[1] + n[1], acc[2] + n[2]]
    });
    let vnorm = norm(vsum);
    let fmt = |v: Option<[f32; 3]>| match v {
        Some(v) => format!("({:+.2}, {:+.2}, {:+.2})", v[0], v[1], v[2]),
        None => "degenerate".to_string(),
    };
    let dot = match (facet, vnorm) {
        (Some(f), Some(n)) => format!("{:+.2}", f[0] * n[0] + f[1] * n[1] + f[2] * n[2]),
        _ => "-".to_string(),
    };
    // A billboard card wound away from its `+X` viewer: the renderer turns its normals round so it
    // is lit off the face it presents. The mean `vnorm` cannot show this, so the shape decides.
    let lit_face = if s.billboard_card_faces_away() {
        "  LIT-FACE-FLIP"
    } else {
        ""
    };
    Some(format!(
        "facet {}  vnorm {}  dot {dot}{lit_face}",
        fmt(facet),
        fmt(vnorm)
    ))
}

/// The bones a batch's vertices ride and, for a billboard batch, whether it is a rigid card. The
/// reference skins each vertex through its own bone weights, so only geometry wholly on the
/// billboard bone turns as one card: `MIXED` counts vertices off it, `SPLIT-W` blended vertices.
fn skin_census(s: &benilla_formats::RenderSubmesh) -> Option<String> {
    if s.joints.is_empty() {
        return None;
    }
    let mut per_bone: std::collections::BTreeMap<u16, usize> = std::collections::BTreeMap::new();
    for j in &s.joints {
        *per_bone.entry(j[0]).or_default() += 1;
    }
    let split = s
        .weights
        .iter()
        .filter(|w| w[0] < 0.999 && w[0] > 0.0)
        .count();
    let bones = per_bone
        .iter()
        .map(|(b, n)| format!("bone {b}×{n}"))
        .collect::<Vec<_>>()
        .join("  ");
    let mut verdict = String::new();
    if let Some(bb) = &s.billboard {
        let off = s.joints.len() - per_bone.get(&bb.bone).copied().unwrap_or(0);
        if off > 0 {
            verdict.push_str(&format!(
                "  MIXED: {off}/{} verts off billboard bone {}",
                s.joints.len(),
                bb.bone
            ));
        }
    }
    if split > 0 {
        verdict.push_str(&format!("  SPLIT-W: {split} verts blended across bones"));
    }
    // Distinct skin tuples and their vertex counts, capped at 8; a rigid card is one at weight 1.
    let mut tuples: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for (j, w) in s.joints.iter().zip(&s.weights) {
        let key = (0..4)
            .filter(|&i| w[i] > 0.0)
            .map(|i| format!("{}@{:.2}", j[i], w[i]))
            .collect::<Vec<_>>()
            .join("+");
        *tuples.entry(key).or_default() += 1;
    }
    let detail = if tuples.len() <= 8 {
        format!(
            "\n      skin tuples: {}",
            tuples
                .iter()
                .map(|(k, n)| format!("[{k}]×{n}"))
                .collect::<Vec<_>>()
                .join("  ")
        )
    } else {
        format!("\n      skin tuples: {} distinct (elided)", tuples.len())
    };
    Some(format!("skin: {bones}{verdict}{detail}"))
}

pub fn m2batch(chain: &mut Chain, internal_path: &str) -> Result<()> {
    let name = normalize(internal_path);
    let data = chain
        .read_file(&name)
        .with_context(|| format!("reading '{name}' from chain"))?;
    let dir = name.rsplit_once('\\').map(|(d, _)| d).unwrap_or("");
    let subs = benilla_formats::parse_m2_render_submeshes(&data, dir, &[])
        .with_context(|| format!("parsing M2 render submeshes '{name}'"))?;
    if let Ok(format) = benilla_m2::parse_m2(&mut std::io::Cursor::new(data.as_slice())) {
        let m = format.model();
        let track = |t: &benilla_m2::M2ScalarTrack| match (t.keys.len(), t.constant()) {
            (0, _) => "keyless".to_string(),
            (_, Some(v)) => format!("const {v:.3}"),
            (n, None) => format!("{n} keys {:.3}..{:.3}", t.keys[0].1, t.keys[n - 1].1),
        };
        let join = |v: Vec<String>| {
            if v.is_empty() {
                "-".to_string()
            } else {
                v.join(", ")
            }
        };
        println!(
            "materials: {}",
            join(
                m.materials
                    .iter()
                    .map(|mt| format!(
                        "flags 0x{:02x}/blend {}",
                        mt.flags.bits(),
                        mt.blend_mode.bits()
                    ))
                    .collect()
            )
        );
        println!(
            "color alpha: {}   transparency: {}   transLookup: {:?}",
            join(m.color_alpha_tracks.iter().map(track).collect()),
            join(m.transparency_tracks.iter().map(track).collect()),
            m.transparency_lookup,
        );
        if let Ok(skin) = m.parse_embedded_skin(&data, 0) {
            // `mat` indexes the materials above, `color` the colour-alpha tracks (`ffff` for none)
            // and `weight` `transLookup`. The reference skips a batch whose
            // `A = instanceAlpha × colors[color].alpha × transparency[transLookup[weight]].weight`
            // is 0 or less (`0x707680`).
            println!("skin batches ({}):", skin.batches().len());
            for (i, b) in skin.batches().iter().enumerate() {
                let w = m
                    .transparency_lookup
                    .get(b.weight_combo_index as usize)
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "-".into());
                println!(
                    "  skin {i:>3}: mat {:>3}  color {:>5}  weight {:>3} -> track {w:>3}  \
                     flags 0x{:02x}/shader 0x{:02x}/texCount {}",
                    b.material_index,
                    b.color_index,
                    b.weight_combo_index,
                    b.flags,
                    b.shader_id,
                    b.texture_count
                );
            }
        }
    }
    println!("{} render batch(es)", subs.len());
    for (i, s) in subs.iter().enumerate() {
        let mut flags = Vec::new();
        if s.emissive {
            flags.push("emissive");
        }
        if s.additive {
            flags.push("additive");
        }
        if s.two_sided {
            flags.push("two-sided");
        }
        if s.no_depth_write {
            flags.push("no-depth-write");
        }
        if s.no_depth_test {
            flags.push("no-depth-test");
        }
        if s.billboard.is_some() {
            flags.push("BILLBOARD");
        }
        // Texcoords generated at runtime (`texture_unit_lookup > 2`): the `uv` line is unused.
        if s.env_map {
            flags.push("ENV-MAP");
        }
        if s.alpha_anim.is_some() {
            flags.push("alpha-anim");
        }
        if s.uv_anim.is_some() {
            flags.push("uv-anim");
        }
        let tex = match (&s.texture, s.char_slot) {
            (Some(t), _) => t.clone(),
            (None, Some(slot)) => format!("<char:{slot:?}>"),
            (None, None) => "NONE".into(),
        };
        let ext = |axis: usize| -> (f32, f32) {
            s.positions
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), p| {
                    (lo.min(p[axis]), hi.max(p[axis]))
                })
        };
        let span = if s.positions.is_empty() {
            "empty".to_string()
        } else {
            let (x, y, z) = (ext(0), ext(1), ext(2));
            format!(
                "span {:.2}x{:.2}x{:.2} @ ({:.2}, {:.2}, {:.2})",
                x.1 - x.0,
                y.1 - y.0,
                z.1 - z.0,
                (x.0 + x.1) / 2.0,
                (y.0 + y.1) / 2.0,
                (z.0 + z.1) / 2.0,
            )
        };
        println!(
            "  batch {i}: geoset {:>4}  {:?}  {} verts  {span}  [{}]  tex {}",
            s.geoset_id,
            s.blend,
            s.positions.len(),
            flags.join(" "),
            tex,
        );
        // The UV extent and its density per yard. A range past 0..1 tiles, and a wrap across a
        // triangle drags its mip to the coarsest level, which dissolves an alpha-keyed cutout.
        if !s.uvs.is_empty() && !s.positions.is_empty() {
            let uext = |axis: usize| {
                s.uvs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), t| {
                    (lo.min(t[axis]), hi.max(t[axis]))
                })
            };
            let (u, v) = (uext(0), uext(1));
            let (du, dv) = (u.1 - u.0, v.1 - v.0);
            let diag = {
                let (x, y, z) = (ext(0), ext(1), ext(2));
                ((x.1 - x.0).powi(2) + (y.1 - y.0).powi(2) + (z.1 - z.0).powi(2)).sqrt()
            };
            let tiles = if du > 1.001 || dv > 1.001 {
                " TILES"
            } else {
                ""
            };
            println!(
                "      uv u[{:+.3}..{:+.3}] v[{:+.3}..{:+.3}]  span {du:.3}x{dv:.3}{tiles}  \
                 uv-per-yard {:.4}",
                u.0,
                u.1,
                v.0,
                v.1,
                if diag > 1e-6 { du.max(dv) / diag } else { 0.0 },
            );
        }
        // A batch whose winding (`facet`) disagrees with its authored normals (`vnorm`) is wound
        // back to front: single-sided, it is culled from the side the author lit.
        if let Some(w) = winding(s) {
            println!("      {w}");
        }
        if let Some(k) = skin_census(s) {
            println!("      {k}");
        }
    }
    Ok(())
}

/// A track's `(lo, hi, held)` over sequence `seq_idx`'s band, by the reference's key search
/// (`0x713d50`): the window is `ranges[seq_idx]`, and a collapsed window (`lo >= hi`) holds
/// `keys[lo]`, so `held` means the band keys nothing.
fn band_span(
    track: &benilla_m2::M2ScalarTrack,
    seq_idx: usize,
    band: (u32, u32),
) -> (f32, f32, bool) {
    let (start, end) = band;
    let in_band: Vec<f32> = track
        .keys
        .iter()
        .filter(|&&(t, _)| t >= start && t <= end)
        .map(|&(_, v)| v)
        .collect();
    if !in_band.is_empty() {
        let lo = in_band.iter().copied().fold(f32::MAX, f32::min);
        let hi = in_band.iter().copied().fold(f32::MIN, f32::max);
        return (lo, hi, false);
    }
    // No key in the band: the reference's window has collapsed and it holds `keys[ranges[i].lo]`.
    let held = track
        .ranges
        .get(seq_idx)
        .and_then(|&(lo, _)| track.keys.get(lo as usize))
        .map(|&(_, v)| v)
        .unwrap_or(1.0);
    (held, held, true)
}

/// Dump an M2's per-sequence material alpha: every colour-alpha and transparency track, then each
/// batch's `colour.alpha × transparency.weight` (the reference's combine, `0x707680`) per sequence
/// band. The reference skips a batch whose factor is 0 or less in that animation.
pub fn m2alpha(chain: &mut Chain, internal_path: &str) -> Result<()> {
    let name = normalize(internal_path);
    let data = chain
        .read_file(&name)
        .with_context(|| format!("reading '{name}' from chain"))?;
    let format = benilla_m2::parse_m2(&mut std::io::Cursor::new(data.as_slice()))
        .map_err(|e| anyhow::anyhow!("parsing M2 '{name}': {e}"))?;
    let m = format.model();
    let Ok(skin) = m.parse_embedded_skin(&data, 0) else {
        println!("no embedded skin — no batches to combine");
        return Ok(());
    };
    // Sequences in file order off the header array (count at 0x1c, offset at 0x20, stride 0x44:
    // u16 anim id at +0, u32 band start and end at +4 and +8), not `parse_m2_animations`, which
    // drops zero-duration sequences: `ranges` is indexed by file slot.
    let seqs: Vec<(u16, (u32, u32))> = {
        let n = u32::from_le_bytes(data[0x1c..0x20].try_into().unwrap_or_default()) as usize;
        let o = u32::from_le_bytes(data[0x20..0x24].try_into().unwrap_or_default()) as usize;
        (0..n)
            .map_while(|i| {
                let e = o + i * 0x44;
                (e + 0x44 <= data.len())
                    .then(|| {
                        (
                            u16::from_le_bytes(data[e..e + 2].try_into().ok()?),
                            (
                                u32::from_le_bytes(data[e + 4..e + 8].try_into().ok()?),
                                u32::from_le_bytes(data[e + 8..e + 12].try_into().ok()?),
                            ),
                        )
                            .into()
                    })
                    .flatten()
            })
            .collect()
    };

    let keys = |t: &benilla_m2::M2ScalarTrack| {
        if t.keys.is_empty() {
            return "keyless".to_string();
        }
        let gs = if t.gseq == 0xffff {
            String::new()
        } else {
            format!("gseq {} ", t.gseq)
        };
        format!(
            "{gs}interp {} · {}\n       ranges: {}",
            t.interp,
            t.keys
                .iter()
                .map(|&(ms, v)| format!("{ms}={v:.3}"))
                .collect::<Vec<_>>()
                .join(" "),
            // The key windows the reference indexes by the playing sequence's file slot.
            if t.ranges.is_empty() {
                "none (whole-track search)".to_string()
            } else {
                t.ranges
                    .iter()
                    .enumerate()
                    .map(|(i, &(lo, hi))| format!("{i}:({lo},{hi})"))
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        )
    };
    println!("colour-alpha tracks ({}):", m.color_alpha_tracks.len());
    for (i, t) in m.color_alpha_tracks.iter().enumerate() {
        println!("  #{i:<3} {}", keys(t));
    }
    println!("transparency tracks ({}):", m.transparency_tracks.len());
    for (i, t) in m.transparency_tracks.iter().enumerate() {
        println!("  #{i:<3} {}", keys(t));
    }

    // As the draw loop resolves them: `colorIndex` indexes `colors[]` directly, and out of range
    // (0xffff included) means no factor; the weight goes through `transLookup`, and only when
    // `textureCount` is non-zero.
    let batches = skin.batches();
    let color_of = |b: &benilla_m2::SkinBatch| m.color_alpha_tracks.get(b.color_index as usize);
    let weight_of = |b: &benilla_m2::SkinBatch| {
        (b.texture_count != 0)
            .then(|| {
                m.transparency_lookup
                    .get(b.weight_combo_index as usize)
                    .and_then(|&t| m.transparency_tracks.get(t as usize))
            })
            .flatten()
    };
    println!("\nper-sequence combined alpha (colour × weight), SKIN-batch columns:");
    print!("{:>4} {:>5} {:>16}", "idx", "anim", "band(ms)");
    for i in 0..batches.len() {
        print!("  {:>13}", format!("b{i}"));
    }
    println!();
    for (i, &(anim_id, band)) in seqs.iter().enumerate() {
        print!("{i:>4} {anim_id:>5} {:>7}..{:<7}", band.0, band.1);
        for b in batches {
            // A gseq track keeps its global sequence's clock, not the band: print its full range.
            let span = |t: Option<&benilla_m2::M2ScalarTrack>| match t {
                None => (1.0, 1.0, false),
                Some(t) if t.keys.is_empty() => (1.0, 1.0, false),
                Some(t) if t.gseq != 0xffff => {
                    let lo = t.keys.iter().map(|&(_, v)| v).fold(f32::MAX, f32::min);
                    let hi = t.keys.iter().map(|&(_, v)| v).fold(f32::MIN, f32::max);
                    (lo, hi, false)
                }
                Some(t) => band_span(t, i, band),
            };
            let (clo, chi, chold) = span(color_of(b));
            let (wlo, whi, whold) = span(weight_of(b));
            let (lo, hi) = (clo * wlo, chi * whi);
            let held = chold && whold;
            let cell = if (hi - lo).abs() < 1e-4 {
                format!("{lo:.3}{}", if held { "*" } else { "" })
            } else {
                format!("{lo:.2}..{hi:.2}")
            };
            // `HIDE`: the combine stays at or below 0 here, and `A ≤ 0` skips the batch.
            let cell = if hi <= 0.0 {
                format!("{cell} HIDE")
            } else {
                cell
            };
            print!("  {cell:>13}");
        }
        println!();
    }
    println!(
        "\n* = the band keys nothing: the value is `keys[ranges[seq].lo]`, the bracket the \
         reference's collapsed key window holds (`0x713d50`)"
    );

    // Our bake per render batch (a billboard batch may split per bone, lengthening the list);
    // disagreeing with the table above is a bake bug, not the art's.
    let dir = name.rsplit_once('\\').map(|(d, _)| d).unwrap_or("");
    let subs = benilla_formats::parse_m2_render_submeshes(&data, dir, &[])
        .with_context(|| format!("parsing M2 render submeshes '{name}'"))?;
    println!("\nas BAKED, per render batch (min..max sampled across each band):");
    print!("{:>4} {:>5} {:>16}", "idx", "anim", "band(ms)");
    for i in 0..subs.len() {
        print!("  {:>13}", format!("r{i}"));
    }
    println!();
    for (i, &(anim_id, band)) in seqs.iter().enumerate() {
        print!("{i:>4} {anim_id:>5} {:>7}..{:<7}", band.0, band.1);
        let period = (band.1.saturating_sub(band.0)) as f32 / 1000.0;
        for sub in &subs {
            let (lo, hi) = match &sub.alpha_anim {
                None => (1.0, 1.0),
                Some(a) => (0..=32)
                    .map(|k| a.sample(Some(i), period * k as f32 / 32.0, 0.0))
                    .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v))),
            };
            let cell = if (hi - lo).abs() < 1e-4 {
                format!("{lo:.3}")
            } else {
                format!("{lo:.2}..{hi:.2}")
            };
            let cell = if hi <= 0.0 {
                format!("{cell} HIDE")
            } else {
                cell
            };
            print!("  {cell:>13}");
        }
        println!();
    }
    Ok(())
}

/// An emitter's file flags as the mechanisms the runtime keys off, plus any unmapped bits.
fn part_flags(flags: u32) -> String {
    const NAMED: [(u32, &str); 14] = [
        // 0x1 is unlit, not lit (`0x70bb00`): an emitter without it takes the scene's light.
        (0x0001, "unlit"),
        // Runtime `rt+0x1ac` bit 0x10 (loader `0x70fd13` → `0x7b5d00`): the reference heap-sorts
        // live particles back to front before the quad writer (`0x7b3a10`); we draw pool order.
        (0x0002, "depthSortParticles(TODO)"),
        // File 0x8 sets `rt+0x194` bit 1 = NOT(0x8) (loader `0x70fd01`), the vertex-format word,
        // not the ride switch; bit 1's reader is untraced, and bit 0 (file 0x1) picks a 4- or
        // 8-word vertex stride. Half the corpus authors 0x8.
        (0x0008, "vertexFormat(rt+0x194 b1, reader unread)"),
        // The ride-or-trail switch (spawn `0x7b8a9a`, draw `0x7b3e6f`): set keeps particles
        // emitter-local, riding the live emitter; clear bakes birth into world space, so a moving
        // host lays a trail `speed × lifetime` long.
        (0x0010, "modelSpace(ride)"),
        (0x0020, "sizeByInstanceScale"),
        (0x0040, "inheritEmitterMotion"),
        (0x0080, "killOutbound(sphere)"),
        (0x0100, "sphereUp(sphere)"),
        (0x0200, "tumbleRandomSign"),
        (0x0400, "tailClampsToAge"),
        (0x1000, "xyQuad"),
        (0x2000, "groundSnap"),
        (0x4000, "followEmitterDelta"),
        (0x8000, "burst"),
    ];
    let mut out: Vec<String> = NAMED
        .iter()
        .filter(|(bit, _)| flags & bit != 0)
        .map(|(_, n)| (*n).to_string())
        .collect();
    let residue = flags & !NAMED.iter().fold(0, |a, (bit, _)| a | bit);
    if residue != 0 {
        out.push(format!("unmapped 0x{residue:x}"));
    }
    if out.is_empty() {
        "none".into()
    } else {
        out.join(" ")
    }
}

/// One baked track's keys as `t=v` pairs (seconds from the slot's band start).
fn keys_str(keys: &[(f32, f32)]) -> String {
    keys.iter()
        .map(|&(t, v)| format!("{t:.3}={v:.3}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Dump one M2's particle emitters in full.
pub fn m2part(chain: &mut Chain, internal_path: &str) -> Result<()> {
    let name = normalize(internal_path);
    let data = chain
        .read_file(&name)
        .with_context(|| format!("reading '{name}' from chain"))?;
    let emitters = benilla_formats::parse_m2_particle_emitters(&data)
        .map_err(|e| anyhow::anyhow!("parsing particle emitters of '{name}': {e}"))?;
    if emitters.is_empty() {
        println!("{name}: no particle emitters");
        return Ok(());
    }
    println!("{name} — {} particle emitter(s)", emitters.len());

    let d = benilla_formats::ParamsNow::default();
    let defaults = [
        d.emission_speed,
        d.speed_variation,
        d.vertical_range,
        d.horizontal_range,
        d.gravity,
        d.lifespan,
        d.area_length,
        d.area_width,
        d.z_source,
    ];

    for (i, e) in emitters.iter().enumerate() {
        let [px, py, pz] = e.position;
        println!(
            "\n#{i:<3} bone {:<4} pos ({px:.3}, {py:.3}, {pz:.3})  {:?} · {:?} · {} · {}",
            e.bone,
            e.shape,
            e.blend,
            match e.head_tail {
                0 => "head",
                1 => "tail",
                _ => "head+tail",
            },
            // Whether the renderer lights this emitter from the scene (`ParticleEmitterDef::lit`).
            if e.lit { "LIT" } else { "unlit" }
        );
        println!(
            "     texture {}  atlas {}x{}  flags 0x{:04x} [{}]",
            e.texture.as_deref().unwrap_or("(none)"),
            e.tile_cols,
            e.tile_rows,
            e.flags,
            part_flags(e.flags)
        );
        if let Some(g) = &e.geometry_model {
            println!("     geometry model (3-D particles): {g}");
        }
        if let Some(r) = &e.recursion_model {
            println!("     recursion model (child emitters): {r}");
        }
        if let Some(s) = &e.spline {
            // Every control point at full precision: the path is `3K+1` points of cubic Bézier,
            // bent by each segment's interior handles, so its ends alone say nothing of its reach.
            let pt = |p: &[f32; 3]| format!("({:.4},{:.4},{:.4})", p[0], p[1], p[2]);
            println!(
                "     spline: {} control points ({} cubic segment(s)), model-local:",
                s.points.len(),
                s.points.len().saturating_sub(1) / 3,
            );
            for (i, chunk) in s.points.chunks(4).enumerate() {
                let row: Vec<String> = chunk.iter().map(pt).collect();
                println!("       [{:>2}] {}", i * 4, row.join("  "));
            }
        }

        match e.timing.constant_rate() {
            Some(r) => println!("     rate {r:.2}/s (same in every sequence)"),
            None => {
                println!(
                    "     rate ANIMATED (peak {:.2}/s), per slot:",
                    e.timing.peak_rate()
                );
                for (slot, (looping, rate, _)) in e.timing.slot_views().iter().enumerate() {
                    if let Some(keys) = rate {
                        println!(
                            "       slot {slot} {} {}",
                            if *looping { "loop" } else { "once" },
                            keys_str(keys)
                        );
                    }
                }
            }
        }
        let gates: Vec<String> = e
            .timing
            .slot_views()
            .iter()
            .enumerate()
            .filter_map(|(slot, (_, _, gate))| {
                gate.map(|keys| format!("slot {slot}: {}", keys_str(keys)))
            })
            .collect();
        println!(
            "     gate {}",
            if gates.is_empty() {
                "always on (keyless)".to_string()
            } else {
                gates.join(" · ")
            }
        );

        // The nine emission parameter tracks: flat ones on one line, moving ones per slot.
        let views = e.params.channel_views();
        let mut flat: Vec<String> = Vec::new();
        let mut animated: Vec<String> = Vec::new();
        for (ch, (label, slots)) in views.iter().enumerate() {
            let vals: Vec<f32> = slots
                .iter()
                .flatten()
                .flat_map(|k| k.iter().map(|&(_, v)| v))
                .collect();
            let (lo, hi) = vals
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
            if vals.is_empty() {
                flat.push(format!("{label} {:.3}*", defaults[ch]));
            } else if (hi - lo).abs() < 1e-6 {
                flat.push(format!("{label} {lo:.3}"));
            } else {
                flat.push(format!("{label} {lo:.3}..{hi:.3} ANIM"));
                for (slot, keys) in slots.iter().enumerate() {
                    if let Some(keys) = keys {
                        animated.push(format!("       {label} slot {slot}: {}", keys_str(keys)));
                    }
                }
            }
        }
        println!(
            "     params: {}   (* = keyless, loader default)",
            flat.join("  ")
        );
        for line in &animated {
            println!("{line}");
        }

        println!(
            "     drag {:.3}  spin {:.3}  tailTime {:.3}  inheritScale {:.3}",
            e.drag, e.spin, e.tail_time, e.inherit_scale
        );
        println!(
            "     twinkle speed {:.3} pct {:.3} range {:.3}..{:.3} [{}]",
            e.twinkle_speed,
            e.twinkle_percent,
            e.twinkle_min,
            e.twinkle_max,
            if (e.twinkle_max - e.twinkle_min).abs() < 1e-6 {
                "degenerate — steady at ramp size"
            } else {
                "active"
            }
        );

        let ol = &e.over_life;
        println!("     over-life (mid {:.3}):", ol.mid);
        for (k, c) in ol.color.iter().enumerate() {
            println!(
                "       colour k{k}  r {:.3} g {:.3} b {:.3}  a {:.3}",
                c[0], c[1], c[2], c[3]
            );
        }
        // Four decimals: UI models author sizes in thousandths, as the autocast shine's 0.0015.
        println!(
            "       size    {:.4} -> {:.4} -> {:.4} yd (half-extent)",
            ol.scale[0], ol.scale[1], ol.scale[2]
        );
        println!(
            "       cells   head A {}->{} B {}->{} · tail A {}->{} B {}->{} · repeat {:.2}/{:.2}",
            ol.head_cells[0].begin,
            ol.head_cells[0].end,
            ol.head_cells[1].begin,
            ol.head_cells[1].end,
            ol.tail_cells[0].begin,
            ol.tail_cells[0].end,
            ol.tail_cells[1].begin,
            ol.tail_cells[1].end,
            ol.repeat[0],
            ol.repeat[1]
        );

        let rate = e.timing.peak_rate();
        // A burst's count is not its peak rate: it fires on the rising edge of
        // `enabled && rate > 0`, and one whose gate closes as its rate opens fires nothing.
        let burst = e.timing.first_burst(Some(0));
        let life = e.params.peak_lifespan();
        let speed = views[0]
            .1
            .iter()
            .flatten()
            .flat_map(|k| k.iter().map(|&(_, v)| v.abs()))
            .fold(0.0f32, f32::max);
        // Drag caps travel near `speed/drag`; without drag a particle coasts for its whole life.
        let reach = if e.drag > 1e-6 {
            (speed / e.drag).min(speed * life)
        } else {
            speed * life
        };
        let size_lo = ol.scale.iter().copied().fold(f32::MAX, f32::min);
        let size_hi = ol.scale.iter().copied().fold(f32::MIN, f32::max);
        println!(
            "     derived: {} · reach ~{reach:.2} yd · size {size_lo:.3}..{size_hi:.3} yd · peak alpha {:.2}",
            if e.burst() {
                match burst {
                    Some((t, n)) => {
                        format!("burst of {n:.0} particles at t={t:.2}s, life {life:.2}s")
                    }
                    None => "NEVER FIRES — the enabled gate never opens while the rate is nonzero"
                        .to_string(),
                }
            } else {
                format!(
                    "steady ~{:.0} live (rate {rate:.1}/s × life {life:.2}s)",
                    rate * life
                )
            },
            ol.color.iter().map(|c| c[3]).fold(0.0f32, f32::max)
        );
    }
    Ok(())
}

/// Dump an M2's camera table in raw file-index order, the index `Model:SetCamera(n)` selects by
/// without `cameraLookup`, which prints beside it because the portrait bake selects through it.
/// Key counts tell a still rig from an authored path (the `Cameras\*.m2` fly-bys).
pub fn m2cam(chain: &mut Chain, internal_path: &str) -> Result<()> {
    let name = normalize(internal_path);
    let data = chain
        .read_file(&name)
        .with_context(|| format!("reading '{name}' from chain"))?;
    let cams = benilla_formats::parse_m2_pane_cameras(&data);
    let lookup = benilla_m2::parse_camera_lookup(&data);
    println!("{name}: {} camera(s), cameraLookup {lookup:?}", cams.len());
    if cams.is_empty() {
        return Ok(());
    }
    println!(
        "idx  type              eye                      target            fov(rad/deg)   near     far     roll   keys p/t/r"
    );
    for (i, c) in cams.iter().enumerate() {
        let s = &c.still;
        let keys = c.tracks.as_deref().map_or([1, 1, 1], |t| {
            [
                t.positions.keys.len(),
                t.target.keys.len(),
                t.roll.keys.len(),
            ]
        });
        println!(
            "{i:>3}  {:>4}  ({:>8.4},{:>8.4},{:>8.4})  ({:>8.4},{:>8.4},{:>8.4})  {:.5}/{:>6.2}  {:>7.4}  {:>7.3}  {:>5.3}  {}/{}/{}{}",
            c.camera_type,
            s.position[0],
            s.position[1],
            s.position[2],
            s.target[0],
            s.target[1],
            s.target[2],
            s.fov,
            s.fov.to_degrees(),
            s.near_clip,
            s.far_clip,
            s.roll,
            keys[0],
            keys[1],
            keys[2],
            if c.tracks.is_some() { "  ANIMATED" } else { "" },
        );
    }
    Ok(())
}
