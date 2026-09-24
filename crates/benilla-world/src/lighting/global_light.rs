//! The one shared global light, as the reference has a single scene light every draw reads.
//!
//! One persistent storage buffer, which every material binds at `storage(90)`: [`build_light_data`]
//! packs the resolved light each frame and [`upload_light`] writes it in place. Material assets
//! are never mutated after creation, since Bevy rebuilds the bind group of a mutated material.

use benilla_formats::LiquidKind;
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_resource::{Buffer, BufferDescriptor, BufferUsages};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::{Render, RenderApp, RenderSystems};

use super::prop_probes::MAX_PROP_PROBES;
use super::{sh, WowLighting};
use crate::dev_state::DebugState;
use crate::view::ViewDistance;
use crate::view::WorldCamera;

/// The shared light, std430-packed as `vec4<f32>` rows (all `vec4`, so std430 equals std140). Every
/// shader that binds the buffer (`wow_model`, `terrain`, `liquid`, `wdl`, `wow_effect`,
/// `static_gx`) mirrors this row order as a prefix; keep them in sync.
///   0 light_ambient (w=Mod2x 1.0) · 1 light_diffuse (w=clamp on) · 2 light_sun (w=dir/SH enable) ·
///   3 light_spec (w=terrain shininess 20) · 4 fog_color (w=enable) ·
///   5 fog_params (x=start y=end w=farclip) · 6-8 sh_c10_{r,g,b} · 9-11 sh_c13_{r,g,b} ·
///   12 sh_c16 · 13-14 water river {shallow,deep} (w=alpha) · 15-16 water ocean (the same) ·
///   17 grade: `.x` the SIDN night fraction, `.yzw` the sun's SH DC at intensity 1 ·
///   18 wmo_fog_color · 19 wmo_fog_params (x=start y=end): the interior fog triple, read by
///      interior-tagged content in place of 4-5 ·
///   20 point_count (x = live entries) · 21+ the point-light table, two rows per light,
///      `[pos.xyz, range]` and `[rgb, 0]`, here because Bevy's clusterable buffer is
///      fragment-only in the view layout and the point term is per vertex.
///
/// The GPU buffer is larger: the interior-prop probes and the skin-palette regions follow this
/// prefix. They stay out of this struct because the extract clones it by value every frame, and
/// ~900 KB overflowed a render thread's stack.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct LightStd430 {
    rows: [[f32; 4]; LIGHT_HEADER_ROWS],
    points: [[f32; 4]; 2 * MAX_POINT_LIGHTS],
}

/// Header row count of the layout above (rows 0..=20), which every light blob sizes against.
pub const LIGHT_HEADER_ROWS: usize = 21;

/// Packs the model-lighting core from `(ambient, diffuse, sun_dir)` and leaves every other lane
/// alone: rows 0-2, the SH block (6-11 and 12 `.xyz`, ambient in the DC `.w` lanes) and the sun's
/// SH DC on 17 `.yzw`, the exterior M2 lane's light. The sun's bands are `Model2.bls`'s
/// `E(n) = D·(3 + 16μ + 15μ²)/34`, μ = n·u toward the light, the interior lane's
/// [`sh::prop_probe_coeffs`] fold: all linear in `D`, so a consumer scales each by the intensity
/// (packed at 1). Producers call this and never copy the layout.
pub fn pack_model_core_rows(
    rows: &mut [[f32; 4]; LIGHT_HEADER_ROWS],
    ambient: [f32; 3],
    diffuse: [f32; 3],
    sun_dir: Vec3,
) {
    rows[0] = [ambient[0], ambient[1], ambient[2], 1.0]; // 0 light_ambient (w=Mod2x 1.0)
    rows[1] = [diffuse[0], diffuse[1], diffuse[2], 1.0]; // 1 light_diffuse (w=clamp on)
    rows[2] = [sun_dir.x, sun_dir.y, sun_dir.z, 1.0]; // 2 light_sun (w=dir/SH enable 1.0)
    let sun = sh::prop_probe_coeffs([0.0; 3], &[(-sun_dir, diffuse)]);
    // The sun folds at intensity 1 without ambient: ambient takes the DC lanes unscaled, and the
    // fold's own DC moves to row 17 `.yzw`, where the shader scales it by the intensity.
    for (i, row) in sun.iter().enumerate().take(6) {
        rows[6 + i] = row.to_array(); // 6-8 sh_c10_{r,g,b} · 9-11 sh_c13_{r,g,b}
    }
    rows[6][3] = ambient[0]; // the DC lanes carry ambient alone
    rows[7][3] = ambient[1];
    rows[8][3] = ambient[2];
    rows[12][0] = sun[6].x; // 12 sh_c16 xyz; .w is free
    rows[12][1] = sun[6].y;
    rows[12][2] = sun[6].z;
    // 17 `.yzw`: the sun's SH DC at intensity 1; `.x` (SIDN) is the scene's.
    rows[17][1] = sun[0].w;
    rows[17][2] = sun[1].w;
    rows[17][3] = sun[2].w;
}

/// The reference commits a point light's diffuse raw, `colour × intensity × modelFade` (composed
/// at `0x716a67`), over-gamut included: `0x71ca80` stores a peak-normalized byte colour and the
/// peak `max(1, r, g, b)`, and `0x593040` decodes them back before the GL light is set. Saturation
/// comes from the per-vertex clamp of the summed lighting, never from the commit. Deviation: the
/// round trip's 8-bit quantization is skipped, because it moves a channel by under 0.5 %.
pub fn commit_raw(rgb: [f32; 3]) -> [f32; 3] {
    rgb.map(|c| c.max(0.0))
}

/// Capacity of the point-light table, fixed in the WGSL mirror structs (keep in sync); past it
/// [`build_light_data`] keeps the lights nearest the camera.
pub(super) const MAX_POINT_LIGHTS: usize = 256;

/// Lights are packed only within this camera distance (yd): a light's effect ends at its 48 yd
/// range, a pool past 300 yd is sub-pixel and fogged, and the cap bounds the per-vertex walk.
const POINT_PACK_RADIUS: f32 = 300.0;

/// The rooms a point light belongs to: a WMO's MOLT fixture (the groups whose MOLR names it) or a
/// prop's M2 light (the groups whose MODR names the prop). The reference admits a WMO's props only
/// in frames the portal walk visits their group (`0x6838f0`, from `0x685d70`), so a culled room's
/// torch registers no light; [`build_light_data`] drops the light while its rooms are culled. A
/// newtype, since a bare [`crate::wmo_portal::WmoGroupVis`] would enlist the light in
/// `apply_model_visibility`.
#[derive(Component)]
pub struct LightRooms(pub(crate) crate::wmo_portal::WmoGroupVis);

/// An authored point light (an M2 light, a WMO MOLT omni, a carried torch) as the packed table
/// reads it. Not a Bevy `PointLight`: nothing here reads clustered lights, and Bevy would run its
/// light passes over every one each frame.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct WorldPointLight {
    /// Linear RGB, hue preserved.
    pub color: [f32; 3],
    /// `4π × authored intensity`, the `PointLight` convention, which the packer's `/(4π)` undoes.
    pub intensity: f32,
    /// The ≤3-nearest selection radius (yd).
    pub range: f32,
}

/// The packed light for this frame, extracted for [`upload_light`].
#[derive(Resource, Clone, Copy, ExtractResource)]
struct WowLightData(LightStd430);

impl Default for WowLightData {
    fn default() -> Self {
        Self(LightStd430 {
            rows: [[0.0; 4]; 21],
            points: [[0.0; 4]; 2 * MAX_POINT_LIGHTS],
        })
    }
}

/// The persistent storage buffer every material binds, created by [`new_shared_light_buffer`] and
/// extracted to the render world; a `Buffer` clone shares the GPU resource.
#[derive(Resource, Clone, ExtractResource)]
pub struct SharedLightBuffer(pub Buffer);

/// Registers the light pack, the probe publish, their extracts and the render-world uploads.
pub(super) fn register(app: &mut App) {
    app.init_resource::<WowLightData>()
        .init_resource::<super::prop_probes::PropProbeExtract>()
        .add_plugins(ExtractResourcePlugin::<WowLightData>::default())
        .add_plugins(ExtractResourcePlugin::<SharedLightBuffer>::default())
        .add_plugins(ExtractResourcePlugin::<super::prop_probes::PropProbeExtract>::default())
        // After transform propagation: a carried light (a torch in a hand) is a child of a moving
        // joint, so its `GlobalTransform` is this frame's only once `Propagate` has run.
        .add_systems(
            PostUpdate,
            build_light_data
                .after(bevy::transform::TransformSystems::Propagate)
                .after(super::update_time_lighting),
        )
        // After the spawners (PostUpdate): publish the probe table for extraction on change.
        .add_systems(PostUpdate, super::prop_probes::publish_prop_probes);
    // A headless build (no GPU, `backends: None`), as the schedule tests use, has no render app.
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.add_systems(
            Render,
            (upload_light, super::prop_probes::upload_prop_probes)
                .in_set(RenderSystems::PrepareResources),
        );
    }
}

/// Creates the shared light buffer, sized by [`light_blob_bytes`], from the main-world
/// `RenderDevice`; the assets foundation builds it at startup.
pub fn new_shared_light_buffer(device: &RenderDevice) -> SharedLightBuffer {
    SharedLightBuffer(device.create_buffer(&BufferDescriptor {
        label: Some("wow_shared_light"),
        size: light_blob_bytes(),
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    }))
}

/// The shared light buffer's full size: the per-frame blob ([`LIGHT_HEADER_ROWS`] rows and the
/// point table), the interior-prop probe region and the skin-palette regions. Every buffer bound
/// as `wow_light` must be this big: wgpu validates the bound size against `wow_model.wgsl`'s
/// whole layout at each draw.
pub fn light_blob_bytes() -> u64 {
    per_frame_blob_bytes()
        + (7 * MAX_PROP_PROBES * 16) as u64
        + crate::rig_palette::palette_regions_bytes()
}

/// The per-frame prefix's size, which is also the probe region's offset.
pub(super) fn per_frame_blob_bytes() -> u64 {
    std::mem::size_of::<LightStd430>() as u64
}

/// Packs the resolved [`WowLighting`], the fog toggle, the farclip and the nearest point lights
/// into the std430 blob.
#[allow(clippy::type_complexity)]
fn build_light_data(
    light: Res<WowLighting>,
    debug: Res<DebugState>,
    view: Res<ViewDistance>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    lights_q: Query<(&WorldPointLight, &GlobalTransform, Option<&LightRooms>)>,
    // The per-frame portal PVS, for the rooms term.
    portals: Query<&crate::wmo_portal::WmoPortalInstance>,
    mut data: ResMut<WowLightData>,
    time: Res<Time>,
    mut last_dump: Local<f64>,
    mut last_rows_dump: Local<f64>,
) {
    let l = &*light;
    let fog_enable = if debug.lighting.disable_fog { 0.0 } else { 1.0 };
    let farclip = view.farclip;
    // River and lake share the non-ocean swatch.
    let (rs, rd, rsa, rda) = l.water_colors(LiquidKind::Still);
    let (os, od, osa, oda) = l.water_colors(LiquidKind::Ocean);
    // Built in a scratch copy and written through `ResMut` only when a row moved: the extract
    // clones this 8.5 KB blob every frame it reads as changed.
    let mut fresh = data.0;
    fresh.rows = [[0.0; 4]; LIGHT_HEADER_ROWS];
    let rows = &mut fresh.rows;
    rows[3] = [l.spec[0], l.spec[1], l.spec[2], 20.0]; // 3 light_spec (w=terrain shininess 20)
    rows[4] = [l.fog_color[0], l.fog_color[1], l.fog_color[2], fog_enable]; // 4 fog_color (w=enable)
    rows[5] = [l.fog_start, l.fog_end, 0.0, farclip]; // 5 fog_params (z unused; w=farclip)
    rows[13] = [rs[0], rs[1], rs[2], rsa]; // 13 water river shallow (w=alpha)
    rows[14] = [rd[0], rd[1], rd[2], rda]; // 14 water river deep
    rows[15] = [os[0], os[1], os[2], osa]; // 15 water ocean shallow
    rows[16] = [od[0], od[1], od[2], oda]; // 16 water ocean deep
    rows[17][0] = l.sidn_night;
    // Row 17 `.x` is the SIDN night fraction and `.yzw` the core packer's; 18/19 are the interior
    // fog triple, and 19.zw and 12.w are free.
    rows[18] = [
        l.wmo_fog_color[0],
        l.wmo_fog_color[1],
        l.wmo_fog_color[2],
        fog_enable,
    ];
    rows[19] = [l.wmo_fog_start, l.wmo_fog_end, 0.0, 0.0];
    // Rows 0-2, 6-12.xyz and 17.yzw: the model-light core.
    pack_model_core_rows(rows, l.ambient, l.diffuse, l.sun_dir);
    // The point table: lights within [`POINT_PACK_RADIUS`], nearest first past capacity. Dividing
    // by 4π undoes the spawn's premultiply, so the colour is the authored `colour × intensity`,
    // committed raw. Entries past the count stay stale; the count row guards every reader.
    let cam_pos = cam.single().map(|t| t.translation()).unwrap_or(Vec3::ZERO);
    let mut pts: Vec<(f32, Vec3, f32, [f32; 3])> = lights_q
        .iter()
        .filter(|(_, _, rooms)| {
            // A culled room's light registers nothing in the reference ([`LightRooms`]); a light
            // that names no rooms always passes.
            crate::wmo_portal::room_admits(
                rooms.map(|r| &r.0),
                rooms.and_then(|r| portals.get(r.0.instance).ok()),
            )
        })
        .filter_map(|(pl, gt, _)| {
            let p = gt.translation();
            let d2 = p.distance_squared(cam_pos);
            (d2 < POINT_PACK_RADIUS * POINT_PACK_RADIUS).then(|| {
                let c = pl.color;
                let s = pl.intensity / (4.0 * std::f32::consts::PI);
                let rgb = commit_raw([c[0] * s, c[1] * s, c[2] * s]);
                (d2, p, pl.range, rgb)
            })
        })
        .collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    pts.truncate(MAX_POINT_LIGHTS);
    fresh.rows[20] = [pts.len() as f32, 0.0, 0.0, 0.0];
    for (i, (_, p, range, rgb)) in pts.iter().enumerate() {
        fresh.points[2 * i] = [p.x, p.y, p.z, *range];
        fresh.points[2 * i + 1] = [rgb[0], rgb[1], rgb[2], 0.0];
    }
    // `WOW_POINTS_DUMP=1` prints the nearest 8 packed lights once a second; `=frame` every frame,
    // which a pool that changes frame to frame needs.
    static POINTS_DUMP: std::sync::OnceLock<Option<std::ffi::OsString>> =
        std::sync::OnceLock::new();
    if let Some(mode) = POINTS_DUMP.get_or_init(|| std::env::var_os("WOW_POINTS_DUMP")) {
        let every = if mode.as_os_str() == "frame" {
            0.0
        } else {
            1.0
        };
        let now = time.elapsed_secs_f64();
        if now - *last_dump >= every {
            *last_dump = now;
            // Candidates for the camera chunk's three slots: the Chebyshev box of `terrain.wgsl`'s
            // `TERRAIN_REACH` (keep in sync), with the 48 yd sphere's count beside it.
            let cell = 533.333_3 / 16.0;
            let half = 32.0 * 533.333_3;
            let snap = |v: f32| (((half + v) / cell).floor() + 0.5) * cell - half;
            let anchor = Vec3::new(snap(cam_pos.x), cam_pos.y, snap(cam_pos.z));
            let (mut boxed, mut sphere) = (0usize, 0usize);
            for (_, p, _, _) in &pts {
                let dv = *p - anchor;
                boxed += usize::from(dv.x.abs().max(dv.z.abs()) <= 33.570_166);
                sphere += usize::from(dv.length() <= 48.0);
            }
            eprintln!(
                "[points] {} packed, cam {cam_pos:.1?} — this chunk's candidates: {boxed} (was {sphere} at the 48 yd sphere), 3 slots",
                pts.len()
            );
            for (d2, p, _, rgb) in pts.iter().take(8) {
                eprintln!(
                    "  d {:6.2}  at [{:8.2},{:7.2},{:8.2}]  rgb [{:.3},{:.3},{:.3}]",
                    d2.sqrt(),
                    p.x,
                    p.y,
                    p.z,
                    rgb[0],
                    rgb[1],
                    rgb[2]
                );
            }
        }
    }
    // `WOW_LIGHT_DUMP=frame` (or `=1` for once a second) prints every packed header row as raw f32
    // bits, so a change below a printed decimal still shows.
    static LIGHT_DUMP: std::sync::OnceLock<Option<std::ffi::OsString>> = std::sync::OnceLock::new();
    if let Some(mode) = LIGHT_DUMP.get_or_init(|| std::env::var_os("WOW_LIGHT_DUMP")) {
        let every = if mode.as_os_str() == "frame" {
            0.0
        } else {
            1.0
        };
        let now = time.elapsed_secs_f64();
        if now - *last_rows_dump >= every {
            *last_rows_dump = now;
            let hash = data
                .0
                .rows
                .iter()
                .flatten()
                .fold(0xcbf2_9ce4_8422_2325u64, |h, v| {
                    (h ^ u64::from(v.to_bits())).wrapping_mul(0x1000_0000_01b3)
                });
            eprintln!("[light] rows {hash:#018x}");
            for (i, r) in fresh.rows.iter().enumerate() {
                eprintln!(
                    "  {i:2} {:08x} {:08x} {:08x} {:08x}   {:9.5} {:9.5} {:9.5} {:9.5}",
                    r[0].to_bits(),
                    r[1].to_bits(),
                    r[2].to_bits(),
                    r[3].to_bits(),
                    r[0],
                    r[1],
                    r[2],
                    r[3],
                );
            }
        }
    }
    if data.0 != fresh {
        data.0 = fresh;
    }
}

/// Render world, in `PrepareResources`: writes the packed light into the shared buffer before any
/// draw reads it.
fn upload_light(
    queue: Res<RenderQueue>,
    buffer: Option<Res<SharedLightBuffer>>,
    data: Option<Res<WowLightData>>,
) {
    let (Some(buffer), Some(data)) = (buffer, data) else {
        return;
    };
    queue.write_buffer(&buffer.0, 0, bytemuck::bytes_of(&data.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `wow_model.wgsl`'s exterior doodad and entity lane over the packed rows gives
    /// `E = A + I·D·(4/17)(0.375 + 2μ + 1.875μ²)`: ambient never scales by the intensity I, and
    /// every sun band scales by it once. `eval_sh_lane` mirrors the WGSL line for line, so a swap
    /// in the shader shows only when the two are read side by side.
    #[test]
    fn the_sh_response_lane_matches_the_closed_form_at_every_intensity() {
        // A committed Stormwind pair at minute ≈1185, from the reference's uploaded constants.
        let ambient = [102.0 / 255.0, 97.0 / 255.0, 123.0 / 255.0];
        let diffuse = [255.0 / 255.0, 112.0 / 255.0, 0.0];
        let sun_dir = Vec3::new(0.31, -0.82, 0.48).normalize(); // travel dir; to-light = −this
        let mut rows = [[0.0f32; 4]; LIGHT_HEADER_ROWS];
        pack_model_core_rows(&mut rows, ambient, diffuse, sun_dir);

        /// The SH branch of `wow_model.wgsl`'s exterior lane, verbatim.
        fn eval_sh_lane(rows: &[[f32; 4]; LIGHT_HEADER_ROWS], n: Vec3, intensity: f32) -> [f32; 3] {
            let quad = [n.x * n.y, n.y * n.z, n.z * n.z, n.x * n.z];
            let x2y2 = n.x * n.x - n.y * n.y;
            let dot3 = |r: [f32; 4]| r[0] * n.x + r[1] * n.y + r[2] * n.z;
            let dot4 =
                |r: [f32; 4]| r[0] * quad[0] + r[1] * quad[1] + r[2] * quad[2] + r[3] * quad[3];
            [0usize, 1, 2].map(|ch| {
                // sh_c10_{r,g,b} = rows[6+ch] (.w = ambient) · sh_c13_{r,g,b} = rows[9+ch]
                // sh_c16.xyz = rows[12][ch] · grade.yzw = rows[17][1+ch] (the sun's DC, at I=1)
                rows[6 + ch][3]
                    + rows[17][1 + ch] * intensity
                    + intensity * (dot3(rows[6 + ch]) + dot4(rows[9 + ch]) + rows[12][ch] * x2y2)
            })
        }

        let u = -sun_dir; // toward-light unit
        let f = |mu: f32| (4.0 / 17.0) * (0.375 + 2.0 * mu + 1.875 * mu * mu);
        // A side-on normal (μ = 0) and a mid-back one (μ ≈ −0.53, the lobe's negative dip).
        let side = u.cross(Vec3::Y).normalize();
        let mid_back = (u * -0.5333 + side * (1.0f32 - 0.5333 * 0.5333).sqrt()).normalize();
        for (label, n) in [
            ("facing", u),
            ("away", -u),
            ("side-on", side),
            ("mid-back", mid_back),
        ] {
            for intensity in [0.5f32, 1.0, 2.5] {
                let mu = n.dot(u);
                let got = eval_sh_lane(&rows, n, intensity);
                for ch in 0..3 {
                    let want = ambient[ch] + intensity * diffuse[ch] * f(mu);
                    assert!(
                        (got[ch] - want).abs() < 1e-5,
                        "{label} I={intensity} ch{ch}: got {} want {want}",
                        got[ch]
                    );
                }
            }
        }
        // At μ = 1 the lobe equals the FFP peak `A + D`, by the 16/17 accumulate scale.
        let peak = eval_sh_lane(&rows, u, 1.0);
        for ch in 0..3 {
            let ffp_peak = ambient[ch] + diffuse[ch]; // ambient + D·max(N·L,0) at N·L = 1
            assert!(
                (peak[ch] - ffp_peak).abs() < 1e-5,
                "peak ch{ch}: SH {} vs FFP {}",
                peak[ch],
                ffp_peak
            );
        }
        // The mid-back dip goes below ambient, the reference's SH ringing; clamping per term
        // instead of the sum would erase it.
        let dip = eval_sh_lane(&rows, mid_back, 1.0);
        assert!(
            dip[0] < ambient[0],
            "mid-back should dip below ambient: {} vs {}",
            dip[0],
            ambient[0]
        );
    }

    /// A held torch, authored `(0.467, 0.290, 0.133) × 3.0`, commits its raw product through the
    /// real packer, red 40% past white (`0x71ca80` encodes, `0x593040` decodes it back).
    #[test]
    fn the_torch_commits_the_raw_authored_product() {
        let mut app = App::new();
        app.init_resource::<WowLighting>()
            .init_resource::<crate::dev_state::DebugState>()
            .init_resource::<crate::view::ViewDistance>()
            .init_resource::<WowLightData>()
            .init_resource::<Time>()
            .add_systems(Update, build_light_data);
        app.world_mut()
            .spawn((crate::view::WorldCamera, GlobalTransform::IDENTITY));
        // The authored torch light, through the spawn recipe.
        app.world_mut().spawn((
            crate::terrain_stream::point_light([0.466_666_7, 0.290_196_1, 0.133_333_34], 3.0),
            GlobalTransform::from_translation(Vec3::new(0.0, 1.5, 0.0)),
        ));
        app.update();

        let rows = &app.world().resource::<WowLightData>().0;
        assert_eq!(rows.rows[20][0], 1.0, "the light packed");
        let rgb = rows.points[1];
        // Over-white is kept: saturation is the receiving vertex's clamp, not the commit's.
        assert!(
            (rgb[0] - 1.400_000_1).abs() < 1e-4,
            "red commits raw past white: {rgb:?}"
        );
        assert!(
            (rgb[1] - 0.870_588_3).abs() < 1e-4,
            "green commits raw: {rgb:?}"
        );
        assert!((rgb[2] - 0.4).abs() < 1e-4, "blue commits raw: {rgb:?}");
    }

    /// The packed SH block gives `Model2.bls`'s `clamp01(A + D·I·(3 + 16μ + 15μ²)/34)` at each
    /// intensity rung (2.5 lit, 1.0 mid-band, 0.5 MCSH-shadowed), with ambient in the DC lanes and
    /// the sun's DC on `grade.yzw`.
    #[test]
    fn exterior_lane_reproduces_the_closed_form_at_every_intensity_rung() {
        let ambient = [0.30, 0.32, 0.38];
        let diffuse = [0.85, 0.70, 0.45];
        let sun_dir = Vec3::new(0.3, -0.8, 0.52).normalize(); // travel dir; to-light = −sun_dir
        let mut rows = [[0.0f32; 4]; LIGHT_HEADER_ROWS];
        pack_model_core_rows(&mut rows, ambient, diffuse, sun_dir);
        // The shader's exterior eval over the packed rows, per channel, at intensity `i`.
        let eval = |n: Vec3, i: f32| -> [f32; 3] {
            let quad = [n.x * n.y, n.y * n.z, n.z * n.z, n.x * n.z];
            [0usize, 1, 2].map(|ch| {
                let c10 = rows[6 + ch];
                let c13 = rows[9 + ch];
                let lin = c10[0] * n.x + c10[1] * n.y + c10[2] * n.z;
                let q: f32 = (0..4).map(|k| c13[k] * quad[k]).sum::<f32>()
                    + rows[12][ch] * (n.x * n.x - n.y * n.y);
                (c10[3] + rows[17][1 + ch] * i + i * (lin + q)).clamp(0.0, 1.0)
            })
        };
        let u = -sun_dir; // toward-light
        let side = u.cross(Vec3::Y).normalize();
        for i in [2.5f32, 1.0, 0.5] {
            for (n, mu, who) in [
                (u, 1.0f32, "facing"),
                (-u, -1.0, "away"),
                (side, 0.0, "side"),
            ] {
                let b = (3.0 + 16.0 * mu + 15.0 * mu * mu) / 34.0;
                let got = eval(n, i);
                for ch in 0..3 {
                    let want = (ambient[ch] + diffuse[ch] * i * b).clamp(0.0, 1.0);
                    assert!(
                        (got[ch] - want).abs() < 1e-5,
                        "I={i} {who}: ch{ch} got {} want {want}",
                        got[ch]
                    );
                }
            }
        }
        // Over the back hemisphere the sun term never dips below the closed form's minimum,
        // −0.0373·C·I at μ ≈ −0.53.
        let zero_amb = {
            let mut r = [[0.0f32; 4]; LIGHT_HEADER_ROWS];
            pack_model_core_rows(&mut r, [0.0; 3], diffuse, sun_dir);
            r
        };
        let eval0 = |n: Vec3, i: f32| -> f32 {
            let quad = [n.x * n.y, n.y * n.z, n.z * n.z, n.x * n.z];
            let c10 = zero_amb[6];
            let c13 = zero_amb[9];
            let lin = c10[0] * n.x + c10[1] * n.y + c10[2] * n.z;
            let q: f32 = (0..4).map(|k| c13[k] * quad[k]).sum::<f32>()
                + zero_amb[12][0] * (n.x * n.x - n.y * n.y);
            zero_amb[6][3] + zero_amb[17][1] * i + i * (lin + q)
        };
        for k in 0..=20 {
            let mu = -1.0 + k as f32 / 20.0;
            let n = (u * mu + side * (1.0 - mu * mu).sqrt()).normalize();
            let floor = -0.0374 * diffuse[0] * 2.5;
            assert!(
                eval0(n, 2.5) >= floor,
                "μ={mu}: ringing {} below the closed-form floor {floor}",
                eval0(n, 2.5)
            );
        }
    }
}
