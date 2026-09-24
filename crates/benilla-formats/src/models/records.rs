//! Raw **authored-record reads** straight off the model bytes — the cosmetic MD20 arrays
//! `benilla-m2` deliberately skips (it parses only the render path) plus the WMO root's MOLT
//! lights: M2 dynamic lights, the WMO fixture lights, and the model's authored
//! **portrait camera** (the unit-frame bake's framing source). Each is a small header-offset walk
//! over the vanilla record shape the reference reads; all positions stay raw WoW model
//! space (the render boundary bakes to Bevy space).

use benilla_bytes::ByteExt;

/// An M2 **light** (MD20 lights array, loaded by `0x70ebd0`). A `light_type == 1` (point) light
/// on a placed prop
/// (campfire/torch/brazier/candle/forge) is committed by the real client as a hardware `GL_LIGHT` with
/// the fixed attenuation `1 / (0.7·d + 0.03·d²)` — constant term 0, so intensity peaks hard at the
/// source: the tight interior "hot-spot". It lights every lit surface — terrain, M2
/// doodads/NPCs, and WMO walls/floors (the committed light is diffuse-only, its ambient
/// and specular are zero *on world props* — the glue background scenes DO author their ambient
/// through one dedicated ambient-only light, so both track pairs are parsed). `position` is model
/// space (WoW axes), relative to `bone` (`-1` = model origin); the spawn site applies the prop
/// transform. Colour/intensity are each track's representative (first) value — per-frame flicker is
/// a later refinement; the authored attenuation start/end are parsed for a cull hint, but the GL
/// curve is fixed and ignores them.
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
    /// The light **bone's local +Z axis in model space** at the track origin (first rotation keys
    /// composed up the parent chain) — a DIRECTIONAL light's direction basis. The byte law
    /// (`0x718a76`): the gather reads **row 2 of the light bone's pose matrix** — never the def
    /// position, which is the POINT branch's input only — and the on-surface **to-light** vector
    /// nets to `normalize(Bz·BM_rot)`. `[0,0,1]` for a boneless/rotationless chain (identity pose).
    pub bone_z: [f32; 3],
    /// The **ON-gating**: both
    /// runtime gate bytes load as `1`, and the per-frame animate loop rewrites the visibility byte
    /// **only when the visibility track has keys** (`716413`: zero keys ⇒ sample skipped ⇒ the load
    /// default 1 stands). So a light is dark iff its asset ships a first visibility key of literally
    /// `0`. `true` = exactly that shape (never cast it); `false` = keyless, or a nonzero first key.
    /// The track's values are **bytes** (`71646d: +0xec = visibilityValue[k0]`), not floats.
    pub visibility_off: bool,
}

impl M2Light {
    /// `type == 1` — an omnidirectional point light (the hot-spot caster). Type 0 (directional) feeds the
    /// ambient accumulator, not a discrete GL light (decision 0016 / `0x71bc70`).
    pub fn is_point(&self) -> bool {
        self.light_type == 1
    }

    /// The spawn gate: a **point** light whose visibility track doesn't hold it dark (`0x716413`).
    /// Every site that turns authored M2 lights into scene lights filters on this.
    pub fn casts(&self) -> bool {
        self.is_point() && !self.visibility_off
    }
}

/// A WMO **MOLT** light (root chunk; byte-verified against real assets — Goldshire inn/forge, Northshire
/// Abbey carry 10/3/42 of them). The artists place omni (`light_type == 0`) lights at the interior
/// fixtures (fireplaces, forge, candles, chandeliers); they are warm-coloured, and each wall fixture
/// doodad gets a companion MOLT at its flames (NSabbey: one per candelabra, ~3.4 yd above the MODD
/// origin). At runtime 1.12 commits them into the scene dynamic-light DB every frame for every visible
/// WMO (`0x695c00` → `0x71b650`), lighting terrain, doodads/NPCs, and the building's own surfaces
/// over their baked MOCV (confirmed live in the reference GL trace: an NSabbey batch
/// drew under `colour × intensity` point lights at the fixed 0/0.7/0.03 falloff). `position` is WMO
/// model space (WoW axes). SMOLight stride 0x30.
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
    /// `type == 0` — an omnidirectional (point) MOLT light: the interior fixture lights. (1 = spot,
    /// 2 = directional, 3 = ambient — none seen in the vanilla human interiors audited.)
    pub fn is_omni(&self) -> bool {
        self.light_type == 0
    }
}

/// First value of a vanilla 28-byte M2Track (values `count@0x14`, `ofs@0x18`) as `f32` / `C3Vector`.
/// The representative (static-or-first-key) value — enough to place + colour a light; per-frame track
/// evaluation (the flame flicker) is layered on later. `None` if the track is empty / out of bounds.
fn track_first_f32(bytes: &[u8], track: usize) -> Option<f32> {
    let nval = bytes.u32_at(track + 0x14)? as usize;
    let ofs = bytes.u32_at(track + 0x18)? as usize;
    if nval == 0 {
        return None;
    }
    bytes.f32_at(ofs)
}
/// First value of a vanilla 28-byte M2Track whose values are **bytes** — the light visibility track
/// ([`M2Light::visibility_off`]). `None` for a keyless track (which the animate loop leaves at the
/// load default, i.e. ON).
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

/// `q · (0,0,1) · q⁻¹` — the +Z axis rotated by `q`, expanded closed-form.
fn quat_rotate_z(q: [f32; 4]) -> [f32; 3] {
    let [x, y, z, w] = q;
    [
        2.0 * (x * z + w * y),
        2.0 * (y * z - w * x),
        1.0 - 2.0 * (x * x + y * y),
    ]
}

/// First key of a vanilla 28-byte M2Track whose values are `[x,y,z,w]` f32 quaternions (the v256
/// bone-rotation key shape). `None` if the track is empty / out of bounds.
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

/// A bone's **model-space +Z axis at the track origin**: first rotation keys composed up the
/// parent chain (`global = parent ∘ local`), applied to `(0,0,1)`. Vanilla bone table
/// `count@0x34`/`ofs@0x38`, record stride `0x6c`: flags `u32@4` (bit `0x04` = keep the model
/// root's orientation — stop inheriting), parent `i16@8`, rotation M2Track `@0x28`. Rotationless
/// chains yield `[0,0,1]` (the identity pose — vanilla's rest skeleton is pure translations).
/// Sequence-0 *animation* of a light bone's rotation is not tracked (no UI_* scene authors one).
fn bone_z_axis(bytes: &[u8], bone: i16) -> [f32; 3] {
    let (Some(count), Some(ofs)) = (bytes.u32_at(0x34), bytes.u32_at(0x38)) else {
        return [0.0, 0.0, 1.0];
    };
    let (count, ofs) = (count as usize, ofs as usize);
    // Collect local rotations leaf→root, cycle-guarded by the bone count.
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

/// One M2 light record at a bounds-checked `rec` (the loop below already proved `rec + 0xd4 <=
/// bytes.len()`), so these reads can only fail on a header/offset bug — treated as "stop", not panic.
/// Offsets as the reference reads them (`0x718960`, `0x714260`): `type@0`, `bone@2`,
/// `position@4`, then 7 × `0x1c` M2Tracks — ambient colour/intensity `@0x10`/`@0x2c`, diffuse
/// colour/intensity `@0x48`/`@0x64`, attenuation start/end `@0x80`/`@0x9c`, visibility `@0xb8`. We
/// take diffuse (colour+intensity) + attenuation + the visibility gate ([`M2Light::visibility_off`],
/// byte-valued); ambient is a later refinement.
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

/// Parse the M2 **lights** array straight from the raw bytes (MD20 `count@0x11c`, `ofs@0x120`, record
/// stride `0xd4`, see [`read_m2_light`]). `benilla-m2` deliberately parses only the render path and
/// skips the cosmetic chunks (lights/particles/tex-anims), so we read the vanilla light record here.
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
            None => break, // unreachable given the guard above; defensive, not a panic
        }
    }
    out
}

/// The model's authored **portrait camera** — the MD20 camera the real 1.12 client renders every
/// unit-frame portrait through: the bake at `0x524f60` selects camera `cameraLookup[0]` and
/// builds `lookAt(position, target, up-from-roll)` + the gxumath *diagonal-FOV* perspective
/// (`0x5c3cc0`, half-angle `(fov/2)/√(aspect²+1)`) at the portrait path's fixed `aspect = 4/3` —
/// net vertical half-angle `0.3·fov` with a 3:4 anamorphic squeeze. The framing is per-model
/// *authored data* — each artist calibrated camera 0 to their own geometry, which is why every
/// ref portrait (human, wolf, rabbit) crops identically tight. No engine-side yaw or
/// normalization exists on top of it. All fields raw WoW model space / radians.
#[derive(Debug, Clone, Copy)]
pub struct M2PortraitCamera {
    /// FOV in radians (record `+0x04`), copied verbatim into the projection build (`0x70ebd0` →
    /// `0x5c3cc0`) — a **diagonal** opening angle in the client's convention, NOT fovy (the
    /// vertical half-angle at the portrait's 4/3 aspect is `0.3·fov`).
    pub fov: f32,
    /// Far clip plane (`+0x08`).
    pub far_clip: f32,
    /// Near clip plane (`+0x0c`).
    pub near_clip: f32,
    /// Camera eye: `position_base (+0x2c)` plus the position track's first key (`+0x10`) — portrait
    /// cameras are static in practice (one key or none), so key 0 *is* the evaluated value.
    pub position: [f32; 3],
    /// Look target: `target_base (+0x54)` plus the target track's first key (`+0x38`).
    pub target: [f32; 3],
    /// Roll about the view axis (radians), the roll track's first key (`+0x60`); `0.0` when unkeyed.
    pub roll: f32,
}

/// Parse the M2's **portrait camera** straight from the raw bytes: cameras `count@0x124`/`ofs@0x128`
/// (stride `0x7c`), selected via `cameraLookup[0]` (`count@0x12c`/`ofs@0x130`, u16 entries) — exactly
/// the real client's selection (see [`M2PortraitCamera`]). `None` when the model carries no camera
/// table / no lookup slot 0 (some props and a few creatures — the caller falls back to heuristic
/// framing). The `0xffff` "none" sentinel and a malformed lookup both fall out as an out-of-range
/// index, which [`parse_m2_camera`] answers with `None`.
pub fn parse_m2_portrait_camera(bytes: &[u8]) -> Option<M2PortraitCamera> {
    let idx = *benilla_m2::parse_camera_lookup(bytes).first()? as usize;
    parse_m2_camera(bytes, idx)
}

/// One camera record by **table index** — the glue screens' `Model:SetCamera(idx)` path (the
/// create/select background scenes carry exactly one camera whose *lookup* slot holds the 0xffff
/// none sentinel, so the portrait selection above sees nothing; the client indexes the table
/// directly there). Same record read as the portrait camera.
/// This is the **key-0 reduction** of [`benilla_m2::parse_cameras`], which is the one place the
/// record's offsets are written down. A pane/portrait camera is static in practice — one key or
/// none — so key 0 *is* its evaluated value; a camera whose tracks actually move over time (the
/// `Cameras\*.m2` cinematic fly-bys) is sampled through the parsed record instead, never here.
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

/// One SMOLight record at a bounds-checked `r` (the loop below already proved `r + 0x30 <= b.len()`):
/// type@0, useAtten@1, colour(BGRA)@4, position@8, intensity@0x14, **attenStart@0x28, attenEnd@0x2c**.
/// The runtime-consumed disk attenuation fields sit at the record's TAIL, not right after the
/// intensity — `+0x18..+0x28` hold four other floats (≈0, −0, −1, −0.5 on every vanilla light
/// audited). Verified two ways: a reference capture's fold window fits
/// exactly the `+0x28/+0x2c` values (NSabbey MOLT[10] 3.3333/4.7222; MOLT[40]/[41]
/// end 5.5556), and the abbey file carries those numbers at `+0x28/+0x2c` while `+0x18/+0x1c` read
/// ≈0/−0 (an earlier read of `+0x18/+0x1c` shipped zeros — dormant while nothing consumed them).
fn read_wmo_light(b: &[u8], r: usize) -> Option<WmoLight> {
    Some(WmoLight {
        light_type: b.u8_at(r)?,
        use_atten: b.u8_at(r + 1)? != 0,
        // CImVector is BGRA in memory; recover RGB.
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

/// Parse a WMO root's **MOLT** lights straight from the chunk bytes (`benilla-wmo` doesn't expose
/// them). Scans the flat root chunk list for `MOLT` (magic stored reversed, `TLOM`) and reads each
/// 0x30-byte SMOLight (see [`read_wmo_light`]). NOTE: the chunk walk must NOT stop at a zero-size
/// chunk — `MOVV(0)`/`MOVB(0)` sit right before MOLT in vanilla roots (a bug there once hid MOLT
/// entirely). `root_bytes` is the raw WMO **root** file.
///
/// Hand-rolled rather than `benilla_bytes::chunks()`: the two disagree on what a
/// record's bound is once a chunk's declared `size` is trusted past this walk. `chunks()` clamps a
/// chunk's *payload slice* to the buffer end and the record loop would then be bounds-checked against
/// that clamped slice; this scan instead advances `o` by the *unclamped* `data + size` (so a corrupt
/// size can push `o` past `b.len()`, which the `while` guard catches next iteration) and, for the
/// matched MOLT chunk, bounds each record against the **whole buffer** (`r + 0x30 <= b.len()`), not
/// against `size`. Both give identical records for well-formed input (the case this function is
/// proven against), but they are not the same algorithm, so this stays hand-rolled per the migration
/// guidance rather than being forced onto `chunks()`.
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
                    None => break, // unreachable given the filter above; defensive, not a panic
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

    /// Build a minimal buffer holding a 2-bone chain (root rotated 90° about +X, child 90° about
    /// +Z locally) with the vanilla layout: bone table `count@0x34`/`ofs@0x38`, stride 0x6c,
    /// rotation track @+0x28 (values `count@+0x14`/`ofs@+0x18`, keys `[x,y,z,w]` f32).
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

    /// GOLDEN — the directional-direction law's rotation walk: `global = parent ∘ local`, applied
    /// to +Z. Root alone: 90° about +X sends +Z → −Y. The child's local 90° about +Z does not move
    /// its own +Z axis, so the chained result equals the root's — proving the composition order
    /// (a local-first walk would differ for the mirrored case below).
    #[test]
    fn bone_z_axis_composes_parent_then_local() {
        let b = two_bone_chain();
        let root = bone_z_axis(&b, 0);
        assert!((root[0]).abs() < 1e-6 && (root[1] + 1.0).abs() < 1e-6 && root[2].abs() < 1e-6);
        let child = bone_z_axis(&b, 1);
        for (c, r) in child.iter().zip(root) {
            assert!((c - r).abs() < 1e-6, "child {child:?} vs root {root:?}");
        }
        // No rotation keys anywhere → identity +Z.
        assert_eq!(bone_z_axis(&vec![0u8; 0x400], 0), [0.0, 0.0, 1.0]);
    }
}

/// One record of the file's **camera table**, by RAW table index — what `Model:SetCamera(n)`
/// selects on a `<Model>` widget.
///
/// The selection is **raw**: `0x76cec0` reads the count off `MD20+0x124` and the record at
/// `[model+0x3c4] + idx·0x84 + 0x80`, and **`cameraLookup` is not consulted** on that path (the
/// sibling pair that *does* consult it, `0x713500`/`0x713540`, has its one call site at the
/// portrait bake). So the index decides and
/// [`Self::camera_type`] never does; it is carried because a reader will otherwise assume the
/// opposite.
///
/// [`Self::still`] is the rest rig — the record's bases plus each track's first key. That is the
/// whole answer for every model a `<Model>` pane can name: a census over all 9691 `.m2` of
/// the composite found every character/creature/interface camera's position/target/roll track
/// carrying exactly one key of `(0,0,0)`/`0`. [`Self::at`] is the general case — the
/// `Cameras\*.m2` fly-bys, whose Bézier paths a widget would sample like any other track.
#[derive(Debug, Clone)]
pub struct M2PaneCamera {
    /// The record's `type` word (`+0x00`): `0` = portrait, `1` = "characterinfo", `-1` on every
    /// shipped fly-by and on the six glue scenes. Never selected on — see the type docs.
    pub camera_type: i32,
    /// The rig at rest: bases + each track's first key, raw WoW model space.
    pub still: M2PortraitCamera,
    /// The authored tracks, kept **only when one of them actually moves** (more than one distinct
    /// key) — otherwise `still` is the camera at every instant and there is nothing to sample.
    pub tracks: Option<Box<M2CameraTracks>>,
}

/// A moving camera's three authored tracks plus their bases — the publish pass's inputs
/// (`0x718960` `[0x718b60, 0x718c3a)`): `eye = position_base + positions(t)`,
/// `target = target_position_base + target(t)`, `roll = roll(t)`.
#[derive(Debug, Clone)]
pub struct M2CameraTracks {
    pub positions: benilla_m2::M2Vec3SplineTrack,
    pub position_base: [f32; 3],
    pub target: benilla_m2::M2Vec3SplineTrack,
    pub target_base: [f32; 3],
    pub roll: benilla_m2::M2ScalarSplineTrack,
}

impl M2PaneCamera {
    /// The rig at absolute file-timeline `ms` — [`Self::still`] for a static camera, the sampled
    /// tracks for a moving one. Raw WoW model space, radians.
    ///
    /// The sampler is [`benilla_m2::M2Track::sample_ms`], the reference's own four-way cubic
    /// `interp` dispatch (STEP/LINEAR/BÉZIER/HERMITE) with both ends clamped — the fly-bys author
    /// BÉZIER. `ms` is the model's animation clock, which for a widget pane is its private scene
    /// clock resolved through the armed sequence's band (the pane's play head plus the band start).
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

/// Parse the whole camera table, in file order — the raw index space `Model:SetCamera(n)` walks.
/// Empty for the overwhelming majority of models (9225 of the composite's 9691).
///
/// This is the table read of [`parse_m2_camera`]'s single-record reduction; both go through
/// [`benilla_m2::parse_cameras`], which is the one place the `0x7c` record layout is written down.
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
            // A track that never leaves its first key is the still rig at every instant; keeping
            // it would cost a sample per frame to reproduce a constant. Every shipped
            // character/creature/interface camera lands here (a census over every shipped `.m2`).
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
