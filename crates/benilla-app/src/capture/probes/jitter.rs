//! `WOW_JITTER=<name-substr>[,<start_s>]`: per frame, the camera, root and pose terms of a
//! subject's rendered position as first and second differences, in mm and in pixels at the
//! subject's distance. Δ² separates noise (≈2n) from smooth motion (≈A/3600 at 60 Hz), and `Δ/dt`
//! beside `dt` separates uneven frame timing from a noisy pose.
//!
//! The `mm` columns are milli-yards (yards × 1000), not millimetres; pixel columns are unaffected.
//!
//! One `JIT` TSV line per frame; the `JIT#` header names the columns when the meter arms.

use bevy::camera::Projection;
use bevy::prelude::*;

use bevy::mesh::skinning::SkinnedMeshInverseBindposes;

use benilla_world::rig_anim::RigPose;
use benilla_world::rig_palette::RigSkin;
use benilla_world::view::WorldCamera;

use super::ProbeClock;
use crate::names::NameCache;
use crate::net::{Guid, NetEntity, SelfPlayer};

/// `WOW_JITTER`'s parsed knobs plus the two-frame history the differences are taken over.
#[derive(Resource)]
struct JitterMeter {
    /// Lower-cased name substring; empty is the nearest rigged non-self unit, `self` the avatar.
    want: String,
    /// Wall-clock second the meter arms.
    at: f32,
    /// Whose history [`Self::prev`] holds; a different entity restarts the series.
    subject: Option<Entity>,
    /// Per-bone model-space translations, last frame and the one before.
    prev: Vec<Vec3>,
    prev2: Vec<Vec3>,
    /// Each bone's matrix applied to a point out along each local axis ([`flesh_radii`]): a
    /// standing idle is nearly pure rotation, which bone origins alone miss.
    prev_flesh: Vec<Vec3>,
    prev2_flesh: Vec<Vec3>,
    /// This subject's per-bone flesh radii ([`flesh_radii`]), computed once.
    radii: Vec<f32>,
    /// Last frame's `(node, seek_time, weight)` per playing animation: `seek_time` is the clock
    /// the pose samples at, where `dt` is the wall clock.
    prev_anim: Vec<(usize, f32, f32)>,
    /// The unit's root and the camera, world space.
    prev_root: Vec3,
    prev2_root: Vec3,
    prev_cam: Vec3,
    prev2_cam: Vec3,
    /// The anchors' world translations (held items, helm, shoulders, cape), in absolute space.
    prev_anc: Vec<Vec3>,
    prev2_anc: Vec<Vec3>,
    /// The rider frames the vertex stage places an attached model with: row-0 translation in the
    /// host's rig frame.
    prev_rid: Vec<Vec3>,
    prev2_rid: Vec<Vec3>,
    /// The palette rows the GPU skins from, probed at each bone's bind position. The palette is
    /// composed again by `rig_anim::compose::rig_worlds` (the `flags & 0x7` arm, billboards), so
    /// a clean `flesh` column does not clear it.
    prev_pal: Vec<Vec3>,
    prev2_pal: Vec<Vec3>,
    /// Each palette probe on screen in pixels, by the shader's own arithmetic
    /// (`view_rot * (p_rig + (origin - cam))`, then clip and viewport).
    prev_scr: Vec<Vec2>,
    prev2_scr: Vec<Vec2>,
    /// The camera's rotation: one ULP of roll shifts the whole scene without any translation.
    prev_camrot: Quat,
    prev2_camrot: Quat,
    /// Each bone's bind position, from the rig's inverse bindposes; computed once.
    binds: Vec<Vec3>,
    /// Frames of history held (`< 2` means Δ² is not defined yet).
    depth: u32,
}

pub(crate) struct JitterMeterPlugin;

impl Plugin for JitterMeterPlugin {
    fn build(&self, app: &mut App) {
        let raw = std::env::var("WOW_JITTER").unwrap_or_default();
        // `<substr>`, `<substr>,<start>`, or a bare `<start>`.
        let (want, at) = match raw.rsplit_once(',') {
            Some((n, t)) => (n.trim(), t.trim().parse::<f32>().unwrap_or(0.0)),
            None => match raw.trim().parse::<f32>() {
                Ok(t) => ("", t),
                Err(_) => (raw.trim(), 0.0),
            },
        };
        app.insert_resource(JitterMeter {
            want: want.to_lowercase(),
            at,
            subject: None,
            prev: Vec::new(),
            prev2: Vec::new(),
            prev_flesh: Vec::new(),
            prev2_flesh: Vec::new(),
            radii: Vec::new(),
            prev_anim: Vec::new(),
            prev_root: Vec3::ZERO,
            prev2_root: Vec3::ZERO,
            prev_cam: Vec3::ZERO,
            prev2_cam: Vec3::ZERO,
            prev_anc: Vec::new(),
            prev2_anc: Vec::new(),
            prev_rid: Vec::new(),
            prev2_rid: Vec::new(),
            prev_pal: Vec::new(),
            prev2_pal: Vec::new(),
            prev_scr: Vec::new(),
            prev2_scr: Vec::new(),
            prev_camrot: Quat::IDENTITY,
            prev2_camrot: Quat::IDENTITY,
            binds: Vec::new(),
            depth: 0,
        })
        // `Last`: after the pose, rig finalize, transform propagation and camera seat have run.
        .add_systems(Last, sample_jitter);
    }
}

/// A leaf bone's assumed flesh radius, model-space yards.
const LEAF_R: f32 = 0.05;
/// The cap on a derived radius, model-space yards.
const MAX_R: f32 = 0.45;

/// Per-bone flesh radius: the distance to its farthest child, clamped to [`LEAF_R`]..[`MAX_R`].
/// Sized per bone so a small bone's rotation is not read at a large bone's lever arm.
fn flesh_radii(parents: &[i16], locals: &[Transform]) -> Vec<f32> {
    let mut r = vec![0.0f32; parents.len()];
    for (child, &p) in parents.iter().enumerate() {
        let Ok(pi) = usize::try_from(p) else { continue };
        if let Some(b) = locals.get(child) {
            if let Some(slot) = r.get_mut(pi) {
                *slot = slot.max(b.translation.length());
            }
        }
    }
    r.iter().map(|&v| v.clamp(LEAF_R, MAX_R)).collect()
}

/// Three probe points per bone, one along each local axis at its [`flesh_radii`] distance.
fn flesh_probes(model: &[bevy::math::Affine3A], radii: &[f32]) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(model.len() * 3);
    for (i, m) in model.iter().enumerate() {
        let r = radii.get(i).copied().unwrap_or(LEAF_R);
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            out.push(m.transform_point3(axis * r));
        }
    }
    out
}

/// What the meter reads per candidate subject.
type SubjectQuery = (
    Entity,
    &'static Guid,
    &'static NetEntity,
    &'static GlobalTransform,
    &'static RigPose,
    Option<&'static RigSkin>,
    Has<SelfPlayer>,
);

/// [`flesh_probes`] on the palette rows. A row is `rig_from_joint × inverse_bindpose`, mapping bind
/// space, so it is probed at the bone's bind position plus the flesh offset.
fn palette_probes(rows: &[Mat4], binds: &[Vec3], radii: &[f32]) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(rows.len() * 3);
    for (i, m) in rows.iter().enumerate() {
        let bind = binds.get(i).copied().unwrap_or(Vec3::ZERO);
        let r = radii.get(i).copied().unwrap_or(LEAF_R);
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            out.push(m.transform_point3(bind + axis * r));
        }
    }
    out
}

/// One `JIT` line per frame for the subject.
fn sample_jitter(
    mut meter: ResMut<JitterMeter>,
    time: ProbeClock,
    names: Res<NameCache>,
    cam: Query<(&GlobalTransform, &Projection), With<WorldCamera>>,
    window: Query<&Window>,
    subjects: Query<SubjectQuery>,
    globals: Query<&GlobalTransform>,
    kids: Query<&Children>,
    meshes: Query<(), With<Mesh3d>>,
    attach: Query<&crate::entities::BoneAttach>,
    riders: Query<&benilla_world::rig_rider::RigRider>,
    palettes: Res<benilla_world::rig_palette::RigPalettes>,
    ibps: Res<Assets<SkinnedMeshInverseBindposes>>,
    players: Query<&AnimationPlayer>,
    // The clock the pose advances on (`benilla_world::frame_pace`), logged beside the wall `dt`.
    vtime: Res<Time<bevy::time::Virtual>>,
) {
    let now = time.elapsed_secs();
    if now < meter.at {
        return;
    }
    let (Ok((cam_g, projection)), Ok(window)) = (cam.single(), window.single()) else {
        return;
    };
    let cam_pos = cam_g.translation();
    // The match nearest the camera.
    let want = meter.want.clone();
    // `WOW_JITTER="self"` meters the avatar; every other filter excludes it.
    let on_self = want == "self";
    let Some((entity, guid, net, tf, rig, skin, _)) = subjects
        .iter()
        .filter(|&(_, guid, _, _, _, _, is_self)| {
            if on_self {
                return is_self;
            }
            !is_self
                && (want.is_empty()
                    || names
                        .peek(guid.0)
                        .is_some_and(|n| n.to_lowercase().contains(&want)))
        })
        .min_by(|a, b| {
            let d = |g: &GlobalTransform| g.translation().distance_squared(cam_pos);
            d(a.3).total_cmp(&d(b.3))
        })
    else {
        return;
    };
    let root = tf.translation();
    let dist = root.distance(cam_pos).max(1.0e-3);
    // Pixels per yard AT THE SUBJECT: the vertical half-frame subtends `dist·tan(fovy/2)` yards.
    let fovy = match projection {
        Projection::Perspective(p) => p.fov,
        _ => return,
    };
    let px_per_yd = (window.physical_height() as f32 * 0.5) / (dist * (fovy * 0.5).tan());
    // The rig composes in model space; the root's scale carries it into the world.
    let scale = tf.scale().max_element();

    let bones: Vec<Vec3> = rig
        .model
        .iter()
        .map(|m| Vec3::from(m.translation))
        .collect();
    if meter.radii.len() != rig.model.len() {
        meter.radii = flesh_radii(&rig.parents, &rig.locals);
    }
    let flesh = flesh_probes(&rig.model, &meter.radii);
    // The palette rows, probed the same way. Empty without a palette slot (bind-pose fallback),
    // which prints as all-zero columns, not a clean result.
    let mut bone_pos: Vec<Vec3> = Vec::new();
    // The same rows' rotation, as the three transformed unit axes.
    let mut bone_axes: Vec<[Vec3; 3]> = Vec::new();
    let pal: Vec<Vec3> = match skin.and_then(|s| {
        Some((
            palettes.rig_rows(s.slot, s.bones() as usize)?,
            ibps.get(s.ibp())?,
        ))
    }) {
        Some((rows, ibp)) => {
            if meter.binds.len() != rows.len() {
                meter.binds = ibp
                    .iter()
                    .take(rows.len())
                    .map(|m| m.inverse().transform_point3(Vec3::ZERO))
                    .collect();
            }
            let probes = palette_probes(&rows, &meter.binds, &meter.radii);
            // Rig-relative bone positions (row applied to the bind point): the raw lane's series.
            bone_pos = rows
                .iter()
                .zip(&meter.binds)
                .map(|(m, &b)| m.transform_point3(b))
                .collect();
            bone_axes = rows
                .iter()
                .map(|m| {
                    [
                        m.transform_vector3(Vec3::X),
                        m.transform_vector3(Vec3::Y),
                        m.transform_vector3(Vec3::Z),
                    ]
                })
                .collect();
            probes
        }
        None => Vec::new(),
    };
    // Project them as `wow_model.wgsl`'s vertex stage does: camera-relative first
    // (`p_rig + (origin - cam)`), then view rotation, clip and viewport.
    let vp = Vec2::new(
        window.physical_width() as f32,
        window.physical_height() as f32,
    );
    let view_rot = Mat3::from_quat(cam_g.rotation()).transpose();
    let clip_from_view = projection.get_clip_from_view();
    let rig_origin = skin.and_then(|s| palettes.slot_origin(s.slot));
    let scr: Vec<Vec2> = match rig_origin {
        Some(origin) => {
            let shift = origin - cam_pos;
            pal.iter()
                .map(|&p| {
                    let clip = clip_from_view * (view_rot * (p + shift)).extend(1.0);
                    if clip.w.abs() < 1.0e-6 {
                        return Vec2::ZERO;
                    }
                    let ndc = Vec2::new(clip.x, clip.y) / clip.w;
                    Vec2::new((ndc.x * 0.5 + 0.5) * vp.x, (0.5 - ndc.y * 0.5) * vp.y)
                })
                .collect()
        }
        None => Vec::new(),
    };
    let camrot = cam_g.rotation();
    // Anchors are scene-graph entities in absolute world space, read as the renderer does.
    let ancs: Vec<Vec3> = rig
        .anchors
        .iter()
        .map(|&(_, e)| globals.get(e).map_or(Vec3::ZERO, |g| g.translation()))
        .collect();
    // The rider frames of every model attached to this unit, in host-rig space, with origins.
    let rids: Vec<(Vec3, Vec3)> = riders
        .iter()
        .filter(|r| r.host == entity)
        .filter_map(|r| palettes.rider_placement(r.slot))
        .collect();
    let dt = time.delta_secs().max(1.0e-6);

    // A new subject or a changed bone count (a re-skin) restarts the series.
    if meter.subject != Some(entity)
        || meter.prev.len() != bones.len()
        || meter.prev_flesh.len() != flesh.len()
        || meter.prev_anc.len() != ancs.len()
        || meter.prev_rid.len() != rids.len()
        || meter.prev_pal.len() != pal.len()
        || meter.prev_scr.len() != scr.len()
    {
        let name = names.peek(guid.0).unwrap_or("?").to_string();
        println!(
            "JIT# t=<s> dt=<ms> dist=<yd> pxyd=<px/yd> | camd1 camd2 (mm) | rootd1 rootd2 (mm) \
             | bone=<max bone> d1 d2 (mm) d1px d2px | flesh=<bone> d1 d2 (mm) d1px d2px v(mm/s) | \
             anc=<n> d1 d2 (mm) d1px d2px | \
             ancstep xyz (mm) | rider=<n> d1 d2 (mm) d1px d2px | \
             pal=<bone> d1 d2 (mm) d1px d2px nbig | \
             SCREEN d1med d2med d2max (px) nbig | camrot d1 d2 (urad) | window={}x{}   subject={name:?} \
             guid={:#x} display={:?} bones={} scale={scale:.3}",
            window.physical_width(),
            window.physical_height(),
            guid.0,
            net.display_id,
            bones.len(),
        );
        // Name the anchors once: bone id and how many meshes hang under each.
        let mut roster = Vec::new();
        for (i, &(bone, e)) in rig.anchors.iter().enumerate() {
            let mut n = 0usize;
            let mut stack = vec![e];
            while let Some(x) = stack.pop() {
                n += usize::from(meshes.contains(x));
                if let Ok(cs) = kids.get(x) {
                    stack.extend(cs.iter());
                }
            }
            roster.push(format!("{i}:bone{bone}/{n}mesh"));
        }
        println!("JIT@ anchors [{}]", roster.join(" "));
        println!(
            "JIT@ riders [{}]",
            riders
                .iter()
                .filter(|r| r.host == entity)
                .map(|r| format!("slot{}@bone{}", r.slot, r.bone))
                .collect::<Vec<_>>()
                .join(" ")
        );
        if let Ok(a) = attach.get(entity) {
            let mut pts: Vec<String> = a
                .points
                .iter()
                .map(|(slot, (bone, _))| format!("slot{slot}=bone{bone}"))
                .collect();
            pts.sort();
            println!("JIT@ attach points [{}]", pts.join(" "));
        }
        meter.subject = Some(entity);
        meter.prev = bones;
        meter.prev_flesh = flesh;
        meter.prev2_flesh = Vec::new();
        meter.prev_anc = ancs;
        meter.prev2_anc = Vec::new();
        meter.prev_rid = rids.iter().map(|&(_, t)| t).collect();
        meter.prev2_rid = Vec::new();
        meter.prev_pal = pal;
        meter.prev2_pal = Vec::new();
        meter.prev_scr = scr;
        meter.prev2_scr = Vec::new();
        meter.prev_camrot = camrot;
        meter.prev2_camrot = camrot;
        meter.prev2 = Vec::new();
        meter.prev_root = root;
        meter.prev2_root = Vec3::ZERO;
        meter.prev_cam = cam_pos;
        meter.prev2_cam = Vec3::ZERO;
        meter.depth = 1;
        return;
    }
    if meter.depth < 2 {
        meter.prev2 = std::mem::take(&mut meter.prev);
        meter.prev = bones;
        meter.prev2_flesh = std::mem::take(&mut meter.prev_flesh);
        meter.prev_flesh = flesh;
        meter.prev2_anc = std::mem::take(&mut meter.prev_anc);
        meter.prev_anc = ancs;
        meter.prev2_rid = std::mem::take(&mut meter.prev_rid);
        meter.prev_rid = rids.iter().map(|&(_, t)| t).collect();
        meter.prev2_pal = std::mem::take(&mut meter.prev_pal);
        meter.prev_pal = pal;
        meter.prev2_scr = std::mem::take(&mut meter.prev_scr);
        meter.prev_scr = scr;
        meter.prev2_camrot = meter.prev_camrot;
        meter.prev_camrot = camrot;
        meter.prev2_root = meter.prev_root;
        meter.prev_root = root;
        meter.prev2_cam = meter.prev_cam;
        meter.prev_cam = cam_pos;
        meter.depth = 2;
        return;
    }

    // Δ and Δ² per bone, in model space, at the worst bone.
    let (mut d2, mut worst) = (0.0f32, 0usize);
    for (i, &p) in bones.iter().enumerate() {
        let c = (p - 2.0 * meter.prev[i] + meter.prev2[i]).length();
        if c > d2 {
            d2 = c;
            worst = i;
        }
    }
    let bd1 = (bones[worst] - meter.prev[worst]).length();
    // The same on the flesh probes, rotation included.
    let (mut fd2, mut fworst) = (0.0f32, 0usize);
    // Per bone, max over its three probes, so the spread below counts bones.
    let mut per_bone = vec![0.0f32; flesh.len() / 3];
    for (i, &p) in flesh.iter().enumerate() {
        let c = (p - 2.0 * meter.prev_flesh[i] + meter.prev2_flesh[i]).length();
        let b = &mut per_bone[i / 3];
        *b = b.max(c);
        if c > fd2 {
            fd2 = c;
            fworst = i;
        }
    }
    let fd1 = (flesh[fworst] - meter.prev_flesh[fworst]).length();
    // One bone or the whole model: `fd2med` is the median bone's Δ², `fnbig` the count past a
    // quarter pixel; a single twitching bone leaves the median at zero.
    let mut sorted = per_bone.clone();
    sorted.sort_by(f32::total_cmp);
    let fd2med = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
    // The matching Δ median; unlike the worst-bone columns, the medians form one series.
    let mut d1s: Vec<f32> = (0..per_bone.len())
        .map(|b| {
            (0..3)
                .map(|k| (flesh[b * 3 + k] - meter.prev_flesh[b * 3 + k]).length())
                .fold(0.0f32, f32::max)
        })
        .collect();
    d1s.sort_by(f32::total_cmp);
    let fd1med = d1s.get(d1s.len() / 2).copied().unwrap_or(0.0);
    let big_px = 0.25 / (scale * px_per_yd).max(1.0e-6);
    let fnbig = per_bone.iter().filter(|&&c| c > big_px).count();
    // The same on the palette probes, without `scale`: the rows already carry the root's frame.
    let (mut pd2, mut pworst) = (0.0f32, 0usize);
    let mut pal_bone = vec![0.0f32; pal.len() / 3];
    for (i, &p) in pal.iter().enumerate() {
        let c = (p - 2.0 * meter.prev_pal[i] + meter.prev2_pal[i]).length();
        let b = &mut pal_bone[i / 3];
        *b = b.max(c);
        if c > pd2 {
            pd2 = c;
            pworst = i;
        }
    }
    let pd1 = pal
        .get(pworst)
        .map_or(0.0, |&p| (p - meter.prev_pal[pworst]).length());
    let pnbig = pal_bone
        .iter()
        .filter(|&&c| c > 0.25 / px_per_yd.max(1.0e-6))
        .count();
    // On screen, in pixels: per bone the max of its three probes, then the median bone, the worst
    // and the count past a quarter pixel.
    let mut scr_bone = vec![(0.0f32, 0.0f32); scr.len() / 3];
    for (i, &q) in scr.iter().enumerate() {
        let c = (q - 2.0 * meter.prev_scr[i] + meter.prev2_scr[i]).length();
        let d = (q - meter.prev_scr[i]).length();
        let b = &mut scr_bone[i / 3];
        *b = (b.0.max(d), b.1.max(c));
    }
    let mut s1: Vec<f32> = scr_bone.iter().map(|b| b.0).collect();
    let mut s2: Vec<f32> = scr_bone.iter().map(|b| b.1).collect();
    s1.sort_by(f32::total_cmp);
    s2.sort_by(f32::total_cmp);
    let sd1med = s1.get(s1.len() / 2).copied().unwrap_or(0.0);
    let sd2med = s2.get(s2.len() / 2).copied().unwrap_or(0.0);
    let sd2max = s2.last().copied().unwrap_or(0.0);
    let snbig = s2.iter().filter(|&&c| c > 0.25).count();
    // Camera rotation in microradians: Δ and Δ² of the angle between consecutive orientations.
    let ang = |a: Quat, b: Quat| a.angle_between(b) * 1.0e6;
    let crd1 = ang(camrot, meter.prev_camrot);
    let crd2 = (ang(camrot, meter.prev_camrot) - ang(meter.prev_camrot, meter.prev2_camrot)).abs();
    // The flesh velocity `Δ/dt`: clean when a smooth pose is sampled at uneven `dt`, ragged when
    // the pose itself is noisy.
    let fvel = fd1 * scale / dt;
    // The same on the anchors, plus the worst one's per-axis step: f32 precision of an absolute
    // coordinate is per axis, so a large coordinate steps on one axis only.
    let (mut ad2, mut aworst) = (0.0f32, 0usize);
    for (i, &p) in ancs.iter().enumerate() {
        let c = (p - 2.0 * meter.prev_anc[i] + meter.prev2_anc[i]).length();
        if c > ad2 {
            ad2 = c;
            aworst = i;
        }
    }
    let ad1 = if ancs.is_empty() {
        0.0
    } else {
        (ancs[aworst] - meter.prev_anc[aworst]).length()
    };
    // The same on the rider rows.
    let (mut rd2, mut rworst) = (0.0f32, 0usize);
    for (i, &(_, t)) in rids.iter().enumerate() {
        let c = (t - 2.0 * meter.prev_rid[i] + meter.prev2_rid[i]).length();
        if c > rd2 {
            rd2 = c;
            rworst = i;
        }
    }
    let rd1 = if rids.is_empty() {
        0.0
    } else {
        (rids[rworst].1 - meter.prev_rid[rworst]).length()
    };
    let astep = if ancs.is_empty() {
        Vec3::ZERO
    } else {
        (ancs[aworst] - meter.prev_anc[aworst]).abs()
    };
    // The animation clock per playing node; a changed node set (replay, `stop_all`) snaps the pose.
    let anim: Vec<(usize, f32, f32)> = players
        .get(entity)
        .map(|p| {
            let mut v: Vec<(usize, f32, f32)> = p
                .playing_animations()
                .map(|(n, a)| (n.index(), a.seek_time(), a.weight()))
                .collect();
            v.sort_by_key(|&(n, ..)| n);
            v
        })
        .unwrap_or_default();
    // The largest Δseek across nodes; NaN when the node set changed.
    let same = anim.len() == meter.prev_anim.len()
        && anim.iter().zip(&meter.prev_anim).all(|(a, b)| a.0 == b.0);
    let dseek = if same {
        anim.iter()
            .zip(&meter.prev_anim)
            .map(|(a, b)| a.1 - b.1)
            .fold(0.0f32, |acc, v| if v.abs() > acc.abs() { v } else { acc })
    } else {
        f32::NAN
    };
    let cam_d1 = (cam_pos - meter.prev_cam).length();
    let cam_d2 = (cam_pos - 2.0 * meter.prev_cam + meter.prev2_cam).length();
    let root_d1 = (root - meter.prev_root).length();
    let root_d2 = (root - 2.0 * meter.prev_root + meter.prev2_root).length();
    // Δ²/dt²: the velocity change the frame applied.
    let dvel = d2 * scale / (dt * dt);
    let mm = |v: f32| v * scale * 1000.0;
    // `ORIGIN`: the two absolute terms the vertex stage sums (`wow_model.wgsl`:
    // `p_cam = frame_from_local·v + (frame_origin - view.world_position)`); at ~9.5 k yards one
    // axis has an f32 ULP of 2^-10 yd.
    if let Some(o) = rig_origin {
        println!(
            "ORIGIN\t{now:.6}\t{:.9}\t{:.9}\t{:.9}\t{:.9}\t{:.9}\t{:.9}",
            o.x, o.y, o.z, cam_pos.x, cam_pos.y, cam_pos.z
        );
    }
    if raw_bone() == Some(usize::MAX) {
        // `WOW_JITTER_RAW=all`: one line per bone per frame, position and axes.
        for (b, p) in bone_pos.iter().enumerate() {
            let a = bone_axes.get(b).copied().unwrap_or_default();
            println!(
                "RAWALL\t{now:.6}\t{b}\t{:.9}\t{:.9}\t{:.9}\t{:.6}\t\
                 {:.9}\t{:.9}\t{:.9}\t{:.9}\t{:.9}\t{:.9}\t{:.9}\t{:.9}\t{:.9}",
                p.x,
                p.y,
                p.z,
                anim.first().map_or(-1.0, |a| a.1),
                a[0].x,
                a[0].y,
                a[0].z,
                a[1].x,
                a[1].y,
                a[1].z,
                a[2].x,
                a[2].y,
                a[2].z
            );
        }
    } else if let Some(b) = raw_bone() {
        // `WOW_JITTER_RAW=<bone>`: one bone's unaggregated series, rig position and screen pixel.
        if let (Some(p), Some(q)) = (bone_pos.get(b), scr.get(3 * b)) {
            println!(
                "RAW\t{now:.6}\t{b}\t{:.9}\t{:.9}\t{:.9}\t{:.6}\t{:.6}\t{:.6}",
                p.x,
                p.y,
                p.z,
                q.x,
                q.y,
                anim.first().map_or(-1.0, |a| a.1)
            );
        }
    }
    println!(
        "JIT\t{now:.4}\t{:.3}\t{dist:.3}\t{px_per_yd:.1}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{worst}\t\
         {:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.1}\t\
         {}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.1}\t{:.4}\t{:.4}\t{}\t{:.3}\t{}\t{:.4}\t{:.3}\t\
         {}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t\
         {:.4}\t{:.4}\t{:.4}\t{}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t\
         {}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{}\t\
         {:.4}\t{:.4}\t{:.4}\t{}\t{:.2}\t{:.2}",
        dt * 1000.0,
        cam_d1 * 1000.0,
        cam_d2 * 1000.0,
        root_d1 * 1000.0,
        root_d2 * 1000.0,
        mm(bd1),
        mm(d2),
        bd1 * scale * px_per_yd,
        d2 * scale * px_per_yd,
        dvel * 1000.0,
        fworst / 3,
        mm(fd1),
        mm(fd2),
        fd1 * scale * px_per_yd,
        fd2 * scale * px_per_yd,
        fvel * 1000.0,
        mm(fd1med),
        mm(fd2med),
        fnbig,
        vtime.delta_secs() * 1000.0,
        anim.len(),
        dseek * 1000.0,
        anim.first().map_or(-1.0, |a| a.2),
        aworst,
        ad1 * 1000.0,
        ad2 * 1000.0,
        ad1 * px_per_yd,
        ad2 * px_per_yd,
        astep.x * 1000.0,
        astep.y * 1000.0,
        astep.z * 1000.0,
        rworst,
        rd1 * 1000.0,
        rd2 * 1000.0,
        rd1 * px_per_yd,
        rd2 * px_per_yd,
        pworst / 3,
        mm(pd1),
        mm(pd2),
        pd1 * px_per_yd,
        pd2 * px_per_yd,
        pnbig,
        sd1med,
        sd2med,
        sd2max,
        snbig,
        crd1,
        crd2,
    );

    meter.prev2 = std::mem::replace(&mut meter.prev, bones);
    meter.prev2_flesh = std::mem::replace(&mut meter.prev_flesh, flesh);
    meter.prev2_pal = std::mem::replace(&mut meter.prev_pal, pal);
    meter.prev2_scr = std::mem::replace(&mut meter.prev_scr, scr);
    meter.prev2_camrot = std::mem::replace(&mut meter.prev_camrot, camrot);
    meter.prev2_anc = std::mem::replace(&mut meter.prev_anc, ancs);
    meter.prev2_rid =
        std::mem::replace(&mut meter.prev_rid, rids.iter().map(|&(_, t)| t).collect());
    meter.prev2_root = std::mem::replace(&mut meter.prev_root, root);
    meter.prev2_cam = std::mem::replace(&mut meter.prev_cam, cam_pos);
    meter.prev_anim = anim;
}

/// `WOW_JITTER_RAW`'s bone (`all` is every bone), read once; unset leaves the raw lane silent.
fn raw_bone() -> Option<usize> {
    static B: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    *B.get_or_init(|| {
        std::env::var("WOW_JITTER_RAW")
            .ok()
            .and_then(|v| match v.trim() {
                "all" => Some(usize::MAX),
                n => n.parse().ok(),
            })
    })
}
