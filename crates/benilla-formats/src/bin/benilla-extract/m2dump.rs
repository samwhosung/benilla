//! Per-model M2 dump printers: `m2coll`, `m2seq`, `m2attach`, `m2anim`, `m2bones`, `m2batch` —
//! the single-model diagnostics that read one `.m2` and print everything a given concern
//! (collision hull, sequences, attachment points, animation channels, bone table, render
//! batches) actually carries.

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

/// Dump every sequence's EVENT keyframes: time (s from sequence start), 4CC ident, payload.
/// The event-order instrument: on attack clips, whether `$CPP` (the victim
/// defense dispatch) precedes `$AH0-3`/`$CAH` (the impact dispatch) decides which of the two
/// mutually-exclusive victim reactions the shared swing record feeds.
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
    // `band` is the sequence's absolute window on the model's global keyframe timeline: every
    // non-global-sequence track selects its keys from it, so it is what says whether a track is
    // actually keyed HERE or is holding a clamped value from some other sequence's band.
    // NB `idx` is this list's index, not the file's: zero-duration sequences are dropped.
    println!(
        "idx  anim   mode   dur(s)   mspd  blend   band(ms)          freq  replay   bones   keys"
    );
    for (i, s) in seqs.iter().enumerate() {
        // How much data the sequence's own time band actually holds: bones with any
        // keyed track, and total keys across T/R/S (clamp constants included — a bone
        // unkeyed in this band pins to its nearest authored key, see `read_bone_track`).
        // Uneven coverage across same-id variations is what exposed the task-#14 tilt
        // (HumanMale Stand idx 136 keys 13 fewer bones than the head).
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
            // `mspd` = the sequence's authored design movement speed (yd/s) — the DIVISOR of the
            // locomotion playback rate (`speed / (mspd · |modelScale|)`, `0x5fe2f0`
            // @0x5fe4be..0x5fe550). `0.00` ⇒ not a locomotion sequence, so it plays at rate 1×.
            s.move_speed,
            // `blend` = the sequence's authored blend-IN time (s) — how long the client cross-fades
            // from the outgoing pose into this one on a blended arm (op4 `blendFlag != 0`, the
            // `+0x98 -> +0xc4` snapshot decayed over `1/blendTime`). `0.000` = an instant cut, and
            // that is what a doll's turn looks like without it.
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
    // The position is raw WoW model space (X forward, Y left, Z up), as the M2 stores it — "where
    // on the model does this rider actually sit?" needs it as much as the bone does: an item glow
    // hangs on ids 0..4, spread along a weapon's length.
    println!("id  bone  position (WoW model space)");
    for a in &attachments {
        let [x, y, z] = a.position;
        println!("{:>2}  {:>4}  [{x:.3} {y:.3} {z:.3}]", a.id, a.bone);
    }
    eprintln!("{} attachment points", attachments.len());
    Ok(())
}

/// One texture-transform track line for the `m2anim` dump: key count, interp/gseq tags, and the
/// first/last keys (enough to read a scroll direction + rate off a waterfall).
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

/// The `m2anim` subcommand's dump — one section per channel family.
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
    // The full defs (pos/blend/texture/shape/rate-keys/ramps) alongside the summary's bone links —
    // which emitter is the flame and which the glow is unreadable from bone+flags alone (the
    // blood-spurt starburst diagnosis, decision 0141; a flame that "doesn't burn" is usually
    // visible right here: an unresolved texture, or a burst rate track whose first key is 0).
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
        // The emission MODEL first: a BURST emitter fires one ftol(rate) puff at its rate edge
        // and never pours — reading its keys as a continuous rate is the exact misdiagnosis
        // behind the Eviscerate 0.5s-vs-2s gap (`0x718ec8` → `0x7b5550`).
        let burst = if d.burst() { "BURST " } else { "" };
        // PER-SEQUENCE timing (the runtime's actual sampling unit — the old print showed the two
        // tracks rebased onto sequence 0's band, which read as authoritative and was exactly the
        // B27 misparse). The quiet case stays quiet: one constant rate, no gate anywhere.
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
        // The enabled gate, per sequence slot (seconds from the slot's band start) — a one-shot
        // effect's choreography, and a state GameObject's "which clips actually fire this".
        let rate = if views.iter().all(|(_, _, e)| e.is_none()) {
            rate // no gate authored anywhere (the overwhelmingly common shape) — no noise
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
        // A tail's streak length is |velocity|·tail_time — without it "how long is this
        // streak" needs a hand-parse of the raw record (the Eviscerate diagnosis gap).
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
        // The parameter channels sample per frame ([`benilla_formats::EmitParams`]); the compact
        // line shows their opening values, and any channel that actually MOVES prints its full
        // keyed ramp below — the view whose absence hid Frost Nova's 0.19 → 13.2 yd emission-
        // radius ride behind a flat "radius [0.19..0.19]".
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
        // The kernel spread (`0x7b8d70`, `0x7b8890`): a sphere's ranges are latitude/longitude
        // about +X (area = min/max shell radius); a plane's are the ±θ/±φ cone about +Z (area =
        // the spawn rectangle). `(lat ±π, lon ±0)` reads directly as the edge-on ring family.
        let spread = match d.shape {
            benilla_formats::ParticleShape::Sphere => format!(
                "radius [{:.2}..{:.2}] lat ±{:.2} lon ±{:.2}",
                now.area_length, now.area_width, now.vertical_range, now.horizontal_range
            ),
            // Spline repurposing (loader `0x70fa5e`–`0x70fae5`): area = tMin/tMax,
            // vRange = tangent-spin ψ, hRange = scatter.
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
        // The per-emitter model references: geometry (3-D model particles) and recursion
        // (child emitters).
        if let Some(g) = &d.geometry_model {
            println!("             MODEL-PARTICLES: {g}");
        }
        if let Some(r) = &d.recursion_model {
            println!("             CHILD-EMITTERS: {r}");
        }
        // The emitter-motion terms (`0x7b5230`): the follow-delta response
        // line's authored (speed → fraction) samples, and the velocity-inherit scale.
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
    // A keyed look track prints its full `(ms, value)` ramp — the value[0]-only display once
    // masked HolySmite's slash ribbons (height keyed 0 → 0.167 → 0 printed as `+0.00`, reading
    // as "no ribbon" when the model authors a flare).
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

    // Texture-transform detail (0130 phase 3 grounding): the parsed TRS tracks plus the
    // batch → lookup → transform wiring, straight from the full parser.
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
    // Color-alpha + texture-weight keys in full (they're tiny scalar tracks): the "how does this
    // effect fade" instrument — the UI cooldown model's finish-flash ramp was pinned from exactly
    // this dump (decision 0137 phase 4).
    if !m.color_alpha_tracks.is_empty() {
        println!("=== color alpha tracks (per M2Color) ===");
        for (i, t) in m.color_alpha_tracks.iter().enumerate() {
            println!("  color {i}: interp {}, keys {:?}", t.interp, t.keys);
        }
    }
    // The RGB half of the same M2Colors: the per-batch tint the client multiplies into the vertex
    // colour. An effect that reads white where the reference reads coloured is usually visible
    // right here (the Frost Nova purple-mist diagnosis).
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
    // Bone SCALE keys per sequence — the "how does this element grow" instrument (the cooldown
    // star's finish-flash pulse is a bone-scale curve, decision 0263's INTERIM). Scale-keyed
    // bones only; effect/UI models keep this tiny.
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
    // `ign` is `flags & 0x7` spelled out — which of the parent's Translate/Scale/Rotate this bone
    // REFUSES, taking the model root's instead. The raw flags hex could always be
    // read for it and never was: three billboard rounds and a mount round each looked at
    // `RidingHorse` bone 30's `0x00000006` and none read it as "the saddle discards the gallop",
    // which decision 0932 then measured as a 21° rider swing and called faithful. A bone-table
    // dump exists to answer "what does this bone actually inherit"; now it says so in words.
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
        // Small tracks get their actual key values — two sequences can share a key COUNT while
        // holding different values (the questgiver-marker seq 0 vs 190 lesson: counts alone
        // mislabeled them "the same").
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
            // Rotation keys as axis-angle (model-space WoW axes, Z up) — the "which way does
            // this element actually turn" instrument: an emitter/billboard orientation bug
            // needs the spin axis as ground truth, which a bare key COUNT never shows. Small
            // tracks print every key; a long track (a swirl/rotor loop) prints first/last plus
            // the axis of the first key-to-key increment — the spin axis itself.
            //
            // A long track also prints `swing` — the largest angle any key makes with the first,
            // i.e. the track's actual AMPLITUDE. `step` is only the first increment, so a summary
            // without this cannot answer "how far does this bone swing", which is the question a
            // "does the rider inherit the mount's gallop pitch" investigation puts to it. Reading
            // `step` as the amplitude under-reports a 20° swing as 1.5°.
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
                    // increment = q1 · q0⁻¹ — its axis is the track's spin axis.
                    let inv0 = [-q0[0], -q0[1], -q0[2], q0[3]];
                    let inc = [
                        q1[3] * inv0[0] + q1[0] * inv0[3] + q1[1] * inv0[2] - q1[2] * inv0[1],
                        q1[3] * inv0[1] - q1[0] * inv0[2] + q1[1] * inv0[3] + q1[2] * inv0[0],
                        q1[3] * inv0[2] + q1[0] * inv0[1] - q1[1] * inv0[0] + q1[2] * inv0[3],
                        q1[3] * inv0[3] - q1[0] * inv0[0] - q1[1] * inv0[1] - q1[2] * inv0[2],
                    ];
                    // The amplitude: the largest angle any key makes with the first. `2·acos|⟨qi,q0⟩|`
                    // is the geodesic angle between two unit quats, sign-folded so a double-cover
                    // flip doesn't read as a 360° swing.
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
            // Scale keys. A bone whose ONLY channel is scale — the pulsing card a spell
            // effect hangs off a billboard, the freezing trap's ice shard — printed nothing
            // here at all: the header row said `S2` and no detail line followed, so "how big
            // does this thing actually get, and in which sequence" could not be answered from
            // the dump. Non-uniform scale is exactly the interesting case (a square card
            // stretched into a column), so all three components print.
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

/// Dump an M2's render batches as the renderer sees them, preceded by the model-level material
/// state the **static visibility cull** reads (`0x707680`: a batch is skipped
/// when `colorAlpha · transparencyWeight ≤ 0`). Batches this dump *lists* are ones that survived
/// that cull — when one of them turns out to be a stray primitive in game, these tables are where
/// the answer has to be, so they print together.
/// The facing readout for one batch (see the call site in [`m2batch`]): the first triangle's
/// winding normal, the mean authored vertex normal, and their dot. `None` for a batch with no
/// triangle to measure.
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
        // A degenerate (zero-area) triangle has no direction to report — say so rather than
        // printing NaNs that read like data.
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
    // A billboard card authored back-to-front against the law's `+X`-at-the-viewer: the renderer
    // turns its normals round so the card is lit off the face it presents. `vnorm`
    // alone cannot answer this — it is the MEAN, so a batch whose normals cancel reads the same as
    // one flat plane — hence the shape's own verdict here rather than an eyeball off the numbers.
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

/// Which BONES a batch's vertices actually ride, and — for a billboard batch — whether the batch is
/// a rigid card at all.
///
/// The reference skins **per vertex**: every vertex is placed by its own (up to four) bone matrices
/// and weights, so a batch that straddles a billboard bone and a static one *deforms* — the static
/// end stays welded to the body while the billboard end swings to the camera. benilla instead splits
/// a batch into per-billboard-bone submeshes keyed on each **triangle's first vertex** and rotates
/// each group rigidly about that bone's pivot. The two agree only when a group's vertices are all
/// 100% on the one billboard bone; where they aren't, we tear geometry off the model that the
/// reference keeps attached. `MIXED` is exactly that condition, and `SPLIT-W` the softer form (a
/// vertex blended across bones, which a rigid group cannot express at all).
fn skin_census(s: &benilla_formats::RenderSubmesh) -> Option<String> {
    if s.joints.is_empty() {
        return None;
    }
    // Vertices per PRIMARY bone (`bone_indices[0]` — the key our batch split groups on), in
    // ascending bone order so two models' lines compare by eye.
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
    // The distinct skin TUPLES with their vertex counts — a rigid group is one tuple at weight 1;
    // anything else names the seam the split has to cut along. Capped so a 300-vert creature batch
    // can't drown the dump (the question only ever has a handful of answers on a card-sized batch).
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
            // One line per batch, because the batch → material/track mapping is the thing an alpha
            // question turns on and it is NOT inferable from the render flags: `mat` indexes the
            // materials list above, `color` the colour-alpha tracks (`ffff` = none), and `weight`
            // indexes `transLookup` → the transparency track. The reference's combine is
            // `A = instanceAlpha × colors[color].alpha × transparency[transLookup[weight]].weight`
            // (`0x707680`), so these three name every input to a batch's
            // visibility.
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
        // This batch's texcoords are GENERATED (`texture_unit_lookup > 2`), so the `uv` line below
        // reports the authored UVs the runtime does *not* read — a degenerate span there is the
        // asset saying "supply these", not a defect in the model.
        if s.env_map {
            flags.push("ENV-MAP");
        }
        if s.alpha_anim.is_some() {
            flags.push("alpha-anim");
        }
        if s.uv_anim.is_some() {
            flags.push("uv-anim");
        }
        // A character runtime slot (body atlas / hair / object / extra skin) has no embedded path —
        // name the slot rather than a misleading bare NONE.
        let tex = match (&s.texture, s.char_slot) {
            (Some(t), _) => t.clone(),
            (None, Some(slot)) => format!("<char:{slot:?}>"),
            (None, None) => "NONE".into(),
        };
        // Model-space extent + centre: a batch that renders as a stray primitive is explained by
        // where and how big it actually is (a degenerate zero-area batch is a different bug from a
        // real card the reference hides some other way).
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
        // The batch's UV extent, and the texel-per-yard density it implies. Two questions this
        // answers that nothing else did: does the batch tile (range outside 0..1 — the sampler
        // repeats, and a wrap discontinuity across a triangle blows up the derivative and drags
        // the sampled mip to the coarsest level), and is the authored density so far above the
        // screen's that even mip 0 is a minification? A cutout batch is where either shows up
        // first, because a coarser mip does not merely soften it — it dissolves the silhouette
        // the alpha key cuts (the Dun Morogh snow-fir report).
        if !s.uvs.is_empty() && !s.positions.is_empty() {
            let uext = |axis: usize| {
                s.uvs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), t| {
                    (lo.min(t[axis]), hi.max(t[axis]))
                })
            };
            let (u, v) = (uext(0), uext(1));
            let (du, dv) = (u.1 - u.0, v.1 - v.0);
            // Model-space diagonal of the batch, as the yard scale the UV span stretches over.
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
        // Which way the geometry FACES, in model space — the question a single-sided batch that
        // renders when it shouldn't (or doesn't when it should) turns on. `facet` is the winding
        // normal of the first triangle (`(p1−p0)×(p2−p0)`, WoW model axes); `vnorm` is the mean
        // authored vertex normal. `dot` compares them: a batch whose winding disagrees with its own
        // authored normals is wound back-to-front, and single-sided (`two_sided` absent above) it is
        // culled from the side the author lit.
        if let Some(w) = winding(s) {
            println!("      {w}");
        }
        // Which bones the batch's vertices ride — and whether a billboard group is a rigid card or
        // a strip we tore off a static neighbour (see [`skin_census`]).
        if let Some(k) = skin_census(s) {
            println!("      {k}");
        }
    }
    Ok(())
}

/// One track's value across sequence `seq_idx`'s band, under the reference's own key-search law
/// (`0x713d50`): the search window is `ranges[seq_idx]`, and a window that
/// collapses (`lo >= hi`) resolves to the single key `keys[lo]`. Returns `(lo, hi, held)` — the
/// value range the batch takes across the band, and whether the band keys nothing (so the value is
/// the bracket hold rather than authored motion).
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

/// Dump an M2's **per-sequence material alpha**: every colour-alpha / transparency track's keys,
/// then the combined per-batch factor (`colour.alpha × transparency.weight`, the reference's
/// combine in `0x707680`) for **every sequence band**, not just the first.
///
/// This is the "which batches does the reference hide, and when" instrument. A batch whose factor
/// is `0` in a band is one the real client **skips entirely** in that animation (`A ≤ 0` culls
/// before the blend mode is read), so a row of zeros under Stand and ones under Death is a batch
/// authored to appear only on death — exactly the voidwalker/banshee shape. `m2batch` gives the
/// batch → track wiring this reads; `m2seq` gives the bands.
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
    // Sequences in **file order**, straight off the header array (count@0x1c/ofs@0x20, stride
    // 0x44: anim id u16 @+0x00, band start/end u32 @+0x04/+0x08). Deliberately NOT
    // `parse_m2_animations`, which drops zero-duration sequences and so renumbers the list — and
    // the per-sequence `ranges` array below is indexed by the FILE slot.
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
            // The per-sequence key windows the reference indexes by the playing sequence's file
            // slot — printed so the `*` hold cells below can be checked against the file itself.
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

    // Each skin batch's two factor tracks, resolved exactly as the draw loop resolves them:
    // colorIndex indexes `colors[]` DIRECTLY (out of range — incl. the 0xffff sentinel — means the
    // factor doesn't apply); textureWeightComboIndex goes through `transLookup` (and applies only
    // when the batch's textureCount is non-zero).
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
            // A gseq-clocked track ignores the sequence band entirely (it runs on the global
            // sequence's own clock), so report its full range rather than a band slice.
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
            // `HIDE` marks a batch the reference never draws in this sequence: the combine can
            // only reach 0 there, and `A ≤ 0` skips the batch outright.
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

    // The same question asked of OUR bake, per RENDER batch — which is not the same index space:
    // a batch spanning several billboard bones splits into one submesh per bone, so `m2batch`'s
    // render list runs longer than the skin list. This half is what the renderer will actually do,
    // so a disagreement with the table above is a bug in the bake, not in the art.
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

/// Decode a particle emitter's file-flag word into the mechanism names the runtime keys off
/// (each is a [`benilla_formats::ParticleEmitterDef`] predicate), plus any bit the loader maps
/// to nothing — an unmapped bit on a model that looks wrong is a lead, not noise.
fn part_flags(flags: u32) -> String {
    const NAMED: [(u32, &str); 14] = [
        // The reference has NO "lit" bit — 0x1 is the UNLIT flag, the inverse of the wiki lore
        // (`0x70bb00`). Naming it the way the binary reads it is the
        // point: an emitter WITHOUT this bit is the one that takes the scene's light, and that
        // silent majority-of-one is what a dump has to make visible.
        (0x0001, "unlit"),
        // Runtime `rt+0x1ac` bit 0x10 (loader `0x70fd13` → `0x7b5d00`): the emitter's live
        // particles are heap-sorted back-to-front on view depth before the quad writer runs
        // (`0x7b3a10`), instead of drawn in pool order. NOT YET IMPLEMENTED — we draw pool
        // order for every emitter; harmless where a cloud is one flat colour (alpha coverage is
        // order-independent), visible where it is not.
        (0x0002, "depthSortParticles(TODO)"),
        // NOT unmapped, and printing it as a lead sent one session hunting it as the ride switch
        // (1578): file `0x8` feeds the SECOND runtime flag word, `rt+0x194` bit 1 = NOT(file 0x8)
        // (loader `0x70fd01`), the vertex-format/blend word, orthogonal to simulation space.
        // Bit 1's own reader is not identified; bit 0's (from file `0x1`) picks a 4- vs 8-word
        // vertex stride. Half the corpus authors it.
        (0x0008, "vertexFormat(rt+0x194 b1, reader unread)"),
        // **The ride-vs-trail switch** (spawn `0x7b8a9a`, draw `0x7b3e6f`): SET
        // stores emitter-LOCAL and re-applies the live emitter matrix at draw (a rigid ride);
        // CLEAR bakes the birth into WORLD and never re-applies it, so a moving host lays a trail
        // `speed × lifetime` long. 30.4% of the corpus, 71% of `Item\ObjectComponents`.
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

/// Dump one M2's particle emitters in full — see the `M2part` command doc.
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
            // The scene-light verdict the renderer will take (`ParticleEmitterDef::lit`) — the
            // difference between "this sheet is shaded by the world" and "this sheet is a
            // full-white cutout", which no other line here shows.
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
            // EVERY control point, and at full precision. This used to print `first .. last`,
            // which is actively misleading on a cubic Bézier: the path is `3K+1` points and the
            // two INTERIOR handles of each segment are what bend it, so an emitter whose ends
            // both sit at the origin can still fling its particles a long way — or not move them
            // at all. Reading "(0,0,0) .. (0,0,0)" as "the particles never travel" is a guess
            // the truncated line invites and cannot support (B228 nearly rested on it).
            // Wrapped at 4 per line so a long path stays readable.
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

        // Emission timing: the rate/gate pair, per file sequence slot.
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

        // The nine emission parameter tracks: the flat ones on one line, the moving ones spelled
        // out per slot (a flattened track is exactly how a spread-out effect
        // collapses to a point).
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
        // 4 decimals, not 3: UI models author their sizes in thousandths, where `{:.3}` turns the
        // autocast shine's 0.0015 mid-key into a printed "0.002" — a 33% error that a transcriber
        // reads as authored truth. The colour ramp above stays at 3 because 0..1 channels
        // do not have that problem.
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

        // The derived read: what this record actually puts on screen.
        let rate = e.timing.peak_rate();
        // A burst emitter's count is NOT its peak rate: the burst fires on the rising edge of
        // `enabled && rate > 0`, and an emitter whose gate closes on the same keyframe its rate
        // opens fires nothing at all. Reading `peak_rate` here reported 50 phantom particles for
        // `Strike_Impact_Chest`'s flare and sent a whole diagnosis 2.7x over on count.
        let burst = e.timing.first_burst(Some(0));
        let life = e.params.peak_lifespan();
        let speed = views[0]
            .1
            .iter()
            .flatten()
            .flat_map(|k| k.iter().map(|&(_, v)| v.abs()))
            .fold(0.0f32, f32::max);
        // Drag caps total travel near `speed/drag` (the integrator's exponential decay); with no
        // drag a particle simply coasts for its lifetime.
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

/// Dump an M2's **camera table**, in raw file-index order — the index space a `<Model>` widget's
/// `Model:SetCamera(n)` walks (the selection is raw, `cameraLookup` is not
/// consulted there, so the table position IS the answer and the record's `type` is not).
///
/// Prints, per record: its `type` word, the eye and look-at target at rest (`base + key 0`, raw
/// WoW model space), the diagonal fov in radians and degrees, near/far, the roll, and the key
/// count of each of the three tracks — the last column being the one that says whether the camera
/// is a still rig (every character/creature/interface camera in the chain) or an authored path
/// (the `Cameras\*.m2` fly-bys). The `cameraLookup` table is printed beside it, because the
/// portrait bake selects through it and the pane does not, and confusing the two is a one-line
/// mistake with a wrong picture at the end of it.
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
