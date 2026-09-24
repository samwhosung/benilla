//! Authored records off the raw model bytes, in raw WoW model space: M2 lights (which `benilla-m2`
//! does not read), the WMO root's MOLT lights, and the M2 camera table.

use benilla_bytes::ByteExt;

/// An M2 light (the MD20 lights array, loaded by `0x70ebd0`), `position` relative to `bone` (`-1`
/// for the origin). The reference commits a point light on a placed prop as a hardware `GL_LIGHT`
/// with the fixed attenuation `1 / (0.7·d + 0.03·d²)`, diffuse only; the glue scenes light their
/// ambient through an ambient-only light, so both pairs are read. Colour and intensity are each
/// track's first value, where the reference animates them; the attenuation range is a cull hint.
#[derive(Debug, Clone, Copy)]
pub struct M2Light {
    pub light_type: u16,
    pub bone: i16,
    pub position: [f32; 3],
    pub ambient_color: [f32; 3],
    pub ambient_intensity: f32,
    pub diffuse_color: [f32; 3],
    pub diffuse_intensity: f32,
    pub attenuation_start: f32,
    pub attenuation_end: f32,
    /// The light bone's +Z axis in model space at the track origin, a directional light's basis:
    /// `0x718a76` reads row 2 of the bone's pose matrix, never the def position.
    pub bone_z: [f32; 3],
    /// Both runtime gate bytes load as 1 and the animate loop rewrites the visibility byte only
    /// when the track has keys (`0x716413`), so a light is dark only when its first visibility key
    /// is 0. The values are bytes (`0x71646d`: `+0xec = visibilityValue[k0]`).
    pub visibility_off: bool,
}

impl M2Light {
    /// A point light; type 0 feeds the ambient accumulator, not a GL light (`0x71bc70`).
    pub fn is_point(&self) -> bool {
        self.light_type == 1
    }

    /// The spawn gate: a point light its visibility track does not hold dark.
    pub fn casts(&self) -> bool {
        self.is_point() && !self.visibility_off
    }
}

/// A WMO MOLT light (`SMOLight`, stride `0x30`), in WMO model space. The 1.12 client commits them
/// to the scene's dynamic lights every frame for every visible WMO (`0x695c00`, `0x71b650`), as
/// `colour × intensity` point lights at the fixed 0/0.7/0.03 falloff over the baked MOCV.
#[derive(Debug, Clone, Copy)]
pub struct WmoLight {
    pub light_type: u8,
    pub use_atten: bool,
    pub color: [f32; 3],
    pub position: [f32; 3],
    pub intensity: f32,
    pub attenuation_start: f32,
    pub attenuation_end: f32,
}

impl WmoLight {
    /// An omni light, the interior fixtures' type (1 is spot, 2 directional, 3 ambient).
    pub fn is_omni(&self) -> bool {
        self.light_type == 0
    }
}

/// The first value of a 28-byte M2Track (values `count@0x14`, `ofs@0x18`).
fn track_first_f32(bytes: &[u8], track: usize) -> Option<f32> {
    let nval = bytes.u32_at(track + 0x14)? as usize;
    let ofs = bytes.u32_at(track + 0x18)? as usize;
    if nval == 0 {
        return None;
    }
    bytes.f32_at(ofs)
}
fn track_first_u8(bytes: &[u8], track: usize) -> Option<u8> {
    let nval = bytes.u32_at(track + 0x14)? as usize;
    let ofs = bytes.u32_at(track + 0x18)? as usize;
    if nval == 0 {
        return None;
    }
    bytes.get(ofs).copied()
}
fn track_first_vec3(bytes: &[u8], track: usize) -> Option<[f32; 3]> {
    let nval = bytes.u32_at(track + 0x14)? as usize;
    let ofs = bytes.u32_at(track + 0x18)? as usize;
    if nval == 0 {
        return None;
    }
    Some([
        bytes.f32_at(ofs)?,
        bytes.f32_at(ofs + 4)?,
        bytes.f32_at(ofs + 8)?,
    ])
}

/// Hamilton product `a·b` of two `[x, y, z, w]` quaternions (the vanilla M2 key layout).
fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// `q · (0,0,1) · q⁻¹`, the +Z axis rotated by `q`, in closed form.
fn quat_rotate_z(q: [f32; 4]) -> [f32; 3] {
    let [x, y, z, w] = q;
    [
        2.0 * (x * z + w * y),
        2.0 * (y * z - w * x),
        1.0 - 2.0 * (x * x + y * y),
    ]
}

fn track_first_quat(bytes: &[u8], track: usize) -> Option<[f32; 4]> {
    let nval = bytes.u32_at(track + 0x14)? as usize;
    let ofs = bytes.u32_at(track + 0x18)? as usize;
    if nval == 0 {
        return None;
    }
    Some([
        bytes.f32_at(ofs)?,
        bytes.f32_at(ofs + 4)?,
        bytes.f32_at(ofs + 8)?,
        bytes.f32_at(ofs + 12)?,
    ])
}

/// A bone's model-space +Z at the track origin, its first rotation keys composed up the parent
/// chain. Bone table `count@0x34`/`ofs@0x38`, stride `0x6c`: flags `u32@4` (`0x04` keeps the
/// root's orientation), parent `i16@8`, rotation `@0x28`. Rotation animation is not applied.
fn bone_z_axis(bytes: &[u8], bone: i16) -> [f32; 3] {
    let (Some(count), Some(ofs)) = (bytes.u32_at(0x34), bytes.u32_at(0x38)) else {
        return [0.0, 0.0, 1.0];
    };
    let (count, ofs) = (count as usize, ofs as usize);
    // Local rotations leaf to root, bounded by the bone count.
    let mut chain: Vec<[f32; 4]> = Vec::new();
    let mut idx = bone;
    for _ in 0..=count {
        if idx < 0 || idx as usize >= count {
            break;
        }
        let rec = ofs + idx as usize * 0x6c;
        if let Some(q) = track_first_quat(bytes, rec + 0x28) {
            chain.push(q);
        }
        let flags = bytes.u32_at(rec + 4).unwrap_or(0);
        if flags & 0x4 != 0 {
            break; // the bone keeps the model root's orientation
        }
        idx = bytes.u16_at(rec + 8).map(|p| p as i16).unwrap_or(-1);
    }
    let mut q = [0.0, 0.0, 0.0, 1.0];
    for local in chain.iter().rev() {
        q = quat_mul(q, *local); // root-first: global = root ∘ … ∘ local
    }
    quat_rotate_z(q)
}

/// One M2 light record at a bounds-checked `rec`, as the reference reads it (`0x718960`,
/// `0x714260`): `type@0`, `bone@2`, `position@4`, then seven `0x1c` M2Tracks: ambient colour and
/// intensity `@0x10`/`@0x2c`, diffuse `@0x48`/`@0x64`, attenuation `@0x80`/`@0x9c`, visibility
/// `@0xb8`.
fn read_m2_light(bytes: &[u8], rec: usize) -> Option<M2Light> {
    let bone = bytes.u16_at(rec + 0x02)? as i16;
    Some(M2Light {
        light_type: bytes.u16_at(rec)?,
        bone,
        bone_z: bone_z_axis(bytes, bone),
        position: [
            bytes.f32_at(rec + 0x04)?,
            bytes.f32_at(rec + 0x08)?,
            bytes.f32_at(rec + 0x0c)?,
        ],
        ambient_color: track_first_vec3(bytes, rec + 0x10).unwrap_or([1.0; 3]),
        ambient_intensity: track_first_f32(bytes, rec + 0x2c).unwrap_or(0.0),
        diffuse_color: track_first_vec3(bytes, rec + 0x48).unwrap_or([1.0; 3]),
        diffuse_intensity: track_first_f32(bytes, rec + 0x64).unwrap_or(1.0),
        attenuation_start: track_first_f32(bytes, rec + 0x80).unwrap_or(0.0),
        attenuation_end: track_first_f32(bytes, rec + 0x9c).unwrap_or(0.0),
        visibility_off: track_first_u8(bytes, rec + 0xb8) == Some(0),
    })
}

/// The M2 lights array (MD20 `count@0x11c`, `ofs@0x120`, stride `0xd4`).
pub fn parse_m2_lights(bytes: &[u8]) -> Vec<M2Light> {
    let (Some(count), Some(ofs)) = (bytes.u32_at(0x11c), bytes.u32_at(0x120)) else {
        return Vec::new();
    };
    let (count, ofs) = (count as usize, ofs as usize);
    let mut out = Vec::with_capacity(count.min(256));
    for i in 0..count {
        let rec = match ofs.checked_add(i * 0xd4) {
            Some(r) if r + 0xd4 <= bytes.len() => r,
            _ => break,
        };
        match read_m2_light(bytes, rec) {
            Some(light) => out.push(light),
            None => break, // unreachable after the guard above
        }
    }
    out
}

/// The model's portrait camera, which the 1.12 client renders every unit-frame portrait through:
/// the bake at `0x524f60` takes camera `cameraLookup[0]` and builds `lookAt(position, target,
/// up-from-roll)` with the diagonal-FOV perspective (`0x5c3cc0`, half-angle `(fov/2)/√(aspect²+1)`)
/// at a fixed 4:3, a vertical half-angle of `0.3·fov`, with nothing on top. Model space, radians.
#[derive(Debug, Clone, Copy)]
pub struct M2PortraitCamera {
    /// The diagonal opening angle, not fovy (`0x70ebd0` to `0x5c3cc0`).
    pub fov: f32,
    pub far_clip: f32,
    pub near_clip: f32,
    /// The eye, like `target` its base plus its track's first key; `roll` is a first key alone.
    pub position: [f32; 3],
    pub target: [f32; 3],
    pub roll: f32,
}

/// The portrait camera, `cameraLookup[0]` (`count@0x12c`/`ofs@0x130`) into the camera table
/// (`count@0x124`/`ofs@0x128`, stride `0x7c`) as the reference selects it; `None` without one,
/// the `0xffff` sentinel included.
pub fn parse_m2_portrait_camera(bytes: &[u8]) -> Option<M2PortraitCamera> {
    let idx = *benilla_m2::parse_camera_lookup(bytes).first()? as usize;
    parse_m2_camera(bytes, idx)
}

/// One camera record by table index, the glue screens' `Model:SetCamera(idx)` path: their one
/// camera has the `0xffff` sentinel in its lookup slot, so the client indexes the table directly.
/// Key 0 of each track; a moving camera is sampled through [`M2PaneCamera::at`].
pub fn parse_m2_camera(bytes: &[u8], index: usize) -> Option<M2PortraitCamera> {
    let cam = benilla_m2::parse_cameras(bytes).into_iter().nth(index)?;
    let base_plus_key = |base: [f32; 3], track: &benilla_m2::M2Vec3SplineTrack| {
        let k = track.keys.first().map_or([0.0; 3], |(_, k)| k.value);
        [base[0] + k[0], base[1] + k[1], base[2] + k[2]]
    };
    Some(M2PortraitCamera {
        fov: cam.fov,
        far_clip: cam.far_clip,
        near_clip: cam.near_clip,
        position: base_plus_key(cam.position_base, &cam.positions),
        target: base_plus_key(cam.target_base, &cam.target),
        roll: cam.roll.keys.first().map_or(0.0, |(_, k)| k.value),
    })
}

/// One `SMOLight` at a bounds-checked `r`: type@0, useAtten@1, colour (BGRA)@4, position@8,
/// intensity@0x14, attenStart@0x28, attenEnd@0x2c. The attenuation the reference uses is the
/// tail; `+0x18..+0x28` hold four other floats (≈0, −0, −1, −0.5 on every vanilla light).
fn read_wmo_light(b: &[u8], r: usize) -> Option<WmoLight> {
    Some(WmoLight {
        light_type: b.u8_at(r)?,
        use_atten: b.u8_at(r + 1)? != 0,
        // CImVector is BGRA in memory.
        color: [
            f32::from(b.u8_at(r + 6)?) / 255.0,
            f32::from(b.u8_at(r + 5)?) / 255.0,
            f32::from(b.u8_at(r + 4)?) / 255.0,
        ],
        position: [b.f32_at(r + 8)?, b.f32_at(r + 12)?, b.f32_at(r + 16)?],
        intensity: b.f32_at(r + 0x14)?,
        attenuation_start: b.f32_at(r + 0x28)?,
        attenuation_end: b.f32_at(r + 0x2c)?,
    })
}

/// A WMO root file's MOLT lights, which `benilla-wmo` does not expose: the chunk list scanned for
/// `MOLT` (stored reversed, `TLOM`). The walk must not stop at a zero-size chunk: `MOVV(0)` and
/// `MOVB(0)` sit right before MOLT in vanilla roots. Hand-rolled, not `benilla_bytes::chunks()`:
/// each record is bounded by the whole buffer, not by a clamped chunk payload.
pub fn parse_wmo_lights(root_bytes: &[u8]) -> Vec<WmoLight> {
    let b = root_bytes;
    let mut o = 0usize;
    while o + 8 <= b.len() {
        let Some(size) = b.u32_at(o + 4).map(|v| v as usize) else {
            break;
        };
        let data = o + 8;
        if &b[o..o + 4] == b"TLOM" || &b[o..o + 4] == b"MOLT" {
            let n = size / 0x30;
            let mut out = Vec::with_capacity(n.min(256));
            for i in 0..n {
                let Some(r) = data.checked_add(i * 0x30).filter(|&r| r + 0x30 <= b.len()) else {
                    break;
                };
                match read_wmo_light(b, r) {
                    Some(light) => out.push(light),
                    None => break, // unreachable after the filter above
                }
            }
            return out;
        }
        o = match data.checked_add(size) {
            Some(x) => x,
            None => break,
        };
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2-bone chain: the root turned 90° about +X, the child 90° about its own +Z.
    fn two_bone_chain() -> Vec<u8> {
        let mut b = vec![0u8; 0x400];
        let put_u32 =
            |b: &mut Vec<u8>, at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        let put_quat = |b: &mut Vec<u8>, at: usize, q: [f32; 4]| {
            for (i, c) in q.iter().enumerate() {
                b[at + 4 * i..at + 4 * i + 4].copy_from_slice(&c.to_le_bytes());
            }
        };
        let bones = 0x100;
        put_u32(&mut b, 0x34, 2);
        put_u32(&mut b, 0x38, bones as u32);
        let s = std::f32::consts::FRAC_1_SQRT_2;
        // Bone 0 (root): parent −1, rotation 90° about +X.
        put_u32(&mut b, bones + 8, 0xffff); // parent i16 −1 (submesh u16 slot unused)
        put_u32(&mut b, bones + 0x28 + 0x14, 1);
        put_u32(&mut b, bones + 0x28 + 0x18, 0x300);
        put_quat(&mut b, 0x300, [s, 0.0, 0.0, s]);
        // Bone 1: parent 0, local rotation 90° about +Z.
        let r1 = bones + 0x6c;
        put_u32(&mut b, r1 + 8, 0); // parent 0
        put_u32(&mut b, r1 + 0x28 + 0x14, 1);
        put_u32(&mut b, r1 + 0x28 + 0x18, 0x340);
        put_quat(&mut b, 0x340, [0.0, 0.0, s, s]);
        b
    }

    /// Root alone: 90° about +X sends +Z to −Y. The child's turn about +Z leaves its own +Z, so
    /// the chain gives the root's axis.
    #[test]
    fn bone_z_axis_composes_parent_then_local() {
        let b = two_bone_chain();
        let root = bone_z_axis(&b, 0);
        assert!((root[0]).abs() < 1e-6 && (root[1] + 1.0).abs() < 1e-6 && root[2].abs() < 1e-6);
        let child = bone_z_axis(&b, 1);
        for (c, r) in child.iter().zip(root) {
            assert!((c - r).abs() < 1e-6, "child {child:?} vs root {root:?}");
        }
        // No rotation keys anywhere: identity +Z.
        assert_eq!(bone_z_axis(&vec![0u8; 0x400], 0), [0.0, 0.0, 1.0]);
    }
}

/// One camera table record by raw index, what `Model:SetCamera(n)` selects on a `<Model>` widget:
/// `0x76cec0` reads the count off `MD20+0x124` and the record at `[model+0x3c4] + idx·0x84 + 0x80`
/// without consulting `cameraLookup` (only `0x713500`/`0x713540` do, for the portrait bake), so
/// the index decides and [`Self::camera_type`] never does. Every shipped character, creature and
/// interface camera is its [`Self::still`] rig, each track one key of zero; [`Self::at`] samples
/// the `Cameras\*.m2` fly-bys.
#[derive(Debug, Clone)]
pub struct M2PaneCamera {
    /// The `type` word (`+0x00`): 0 portrait, 1 characterinfo, -1 on every shipped fly-by and the
    /// six glue scenes.
    pub camera_type: i32,
    /// The rig at rest: the bases plus each track's first key.
    pub still: M2PortraitCamera,
    /// The authored tracks, kept only when one moves (more than one distinct key).
    pub tracks: Option<Box<M2CameraTracks>>,
}

/// A moving camera's tracks and bases, the publish pass's inputs (`0x718960`,
/// `[0x718b60, 0x718c3a)`): `eye = position_base + positions(t)`, likewise the target.
#[derive(Debug, Clone)]
pub struct M2CameraTracks {
    pub positions: benilla_m2::M2Vec3SplineTrack,
    pub position_base: [f32; 3],
    pub target: benilla_m2::M2Vec3SplineTrack,
    pub target_base: [f32; 3],
    pub roll: benilla_m2::M2ScalarSplineTrack,
}

impl M2PaneCamera {
    /// The rig at absolute file-timeline `ms`, through [`benilla_m2::M2Track::sample_ms`] (the
    /// reference's four-way `interp` dispatch, both ends clamped; the fly-bys author Bézier). A
    /// pane's `ms` is its play head plus the armed sequence's band start.
    pub fn at(&self, ms: u32) -> M2PortraitCamera {
        let Some(t) = self.tracks.as_deref() else {
            return self.still;
        };
        let add = |b: [f32; 3], v: Option<[f32; 3]>| {
            let v = v.unwrap_or([0.0; 3]);
            [b[0] + v[0], b[1] + v[1], b[2] + v[2]]
        };
        M2PortraitCamera {
            position: add(t.position_base, t.positions.sample_ms(ms)),
            target: add(t.target_base, t.target.sample_ms(ms)),
            roll: t.roll.sample_ms(ms).unwrap_or(self.still.roll),
            ..self.still
        }
    }
}

/// The whole camera table in file order, the index space of `Model:SetCamera(n)`; empty for most
/// models. The `0x7c` record layout lives in [`benilla_m2::parse_cameras`].
pub fn parse_m2_pane_cameras(bytes: &[u8]) -> Vec<M2PaneCamera> {
    benilla_m2::parse_cameras(bytes)
        .into_iter()
        .map(|cam| {
            let key0 = |t: &benilla_m2::M2Vec3SplineTrack| {
                t.keys.first().map_or([0.0; 3], |(_, k)| k.value)
            };
            let base_plus = |b: [f32; 3], t: &benilla_m2::M2Vec3SplineTrack| {
                let k = key0(t);
                [b[0] + k[0], b[1] + k[1], b[2] + k[2]]
            };
            let still = M2PortraitCamera {
                fov: cam.fov,
                far_clip: cam.far_clip,
                near_clip: cam.near_clip,
                position: base_plus(cam.position_base, &cam.positions),
                target: base_plus(cam.target_base, &cam.target),
                roll: cam.roll.keys.first().map_or(0.0, |(_, k)| k.value),
            };
            // A track that never leaves its first key is the still rig at every instant.
            let moves = |n: usize, same: bool| n > 1 && !same;
            let v3_moves = |t: &benilla_m2::M2Vec3SplineTrack| {
                let first = key0(t);
                moves(t.keys.len(), t.keys.iter().all(|(_, k)| k.value == first))
            };
            let roll_moves = {
                let first = cam.roll.keys.first().map_or(0.0, |(_, k)| k.value);
                moves(
                    cam.roll.keys.len(),
                    cam.roll.keys.iter().all(|(_, k)| k.value == first),
                )
            };
            let tracks =
                (v3_moves(&cam.positions) || v3_moves(&cam.target) || roll_moves).then(|| {
                    Box::new(M2CameraTracks {
                        positions: cam.positions,
                        position_base: cam.position_base,
                        target: cam.target,
                        target_base: cam.target_base,
                        roll: cam.roll,
                    })
                });
            M2PaneCamera {
                camera_type: cam.camera_type,
                still,
                tracks,
            }
        })
        .collect()
}
