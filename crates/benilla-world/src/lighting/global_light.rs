//! The **one shared global light** — the faithful replacement for the per-material light copy.
//!
//! The real 1.12 client has a single scene light every draw reads; updating it is O(1). We used to
//! store the resolved [`super::WowLighting`] *per material* and re-push it into every loaded terrain/
//! model/liquid/wdl material each frame — Bevy then freed+recreated every material's bind group
//! (`bevy_pbr` material.rs has an explicit "no fast path; we delete and recreate" TODO), a confirmed
//! ~40fps tax.
//!
//! Instead: ONE persistent GPU storage buffer, created once from the main-world [`RenderDevice`]; every
//! material references it via `#[storage(90, read_only, buffer)]` (a pre-made `Buffer`, baked into the
//! bind group at prepare, zero per-frame upload — `#[uniform]` can't do this, it always re-allocates). The
//! material assets are **never mutated after creation**, so no bind group is ever rebuilt. Each frame
//! [`build_light_data`] (main world) packs the resolved light into a std430 blob and [`upload_light`]
//! (render world, `PrepareResources`) writes it in place — all bind groups see the new data, zero
//! rebuilds, regardless of how many tiles/models are loaded or how fast the clock moves.

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

/// The shared light, std430-packed as contiguous `vec4<f32>` rows. All-`vec4` so std430 == std140
/// (each row 16-aligned, no stride surprises). The row order is the canonical layout every shader
/// bound at `storage(90)` mirrors as a prefix — the WGSL structs in `wow_model.wgsl`/`terrain.wgsl`/
/// `wow_effect.wgsl`. (`liquid.wgsl`/`wdl.wgsl` reuse the field NAMES but bind their own
/// per-material uniforms fed by `apply_wow_lighting` — editing this layout does NOT reach them.)
///   0 light_ambient (w=Mod2x 1.0) · 1 light_diffuse (w=clamp on) · 2 light_sun (w=dir/SH enable) ·
///   3 light_spec (w=terrain shininess 20) · 4 fog_color (w=enable) · 5 fog_params (x=start y=end w=farclip) ·
///   6-8 sh_c10_{r,g,b} · 9-11 sh_c13_{r,g,b} · 12 sh_c16 ·
///   13-14 water river {shallow,deep} (w=alpha) · 15-16 water ocean {shallow,deep} (w=alpha) ·
///   17 grade — `.x` = the SIDN night fraction (`WowLighting::sidn_night`: 1 overnight, 0 all day;
///      `wow_model.wgsl` scales every WMO SIDN material's authored emissive by it); `.yzw` = the
///      sun's isotropic SH DC term at intensity 1 (the exterior M2 lane scales it per instance) ·
///   18 wmo_fog_color · 19 wmo_fog_params (x=start y=end) — the INTERIOR fog triple (the 4 s
///      camera-in-WMO MFOG crossfade; == the scene fog outdoors). Read only by `wow_model.wgsl`'s
///      interior lanes (`0x71c110` stages it into the model's own light collector); terrain
///      mirrors the rows for layout only ·
///   20 point_count (x = live entries) · 21+ the point-light table, TWO rows per light:
///      `[pos.xyz, range]`, `[rgb, 0]` (decision 0278 — the Gouraud point term reads this in the
///      VERTEX stage; bevy's own clusterable buffer is fragment-only in the view bind-group layout,
///      so the lights ride our buffer instead).
///
/// The GPU buffer is LARGER than this per-frame blob: the interior-prop probe table
/// (`lighting::prop_probes`, 7 rows per slot) lives at its tail — [`light_blob_bytes`] sizes the
/// buffer for both, `upload_light` rewrites only this prefix each frame, and `upload_prop_probes`
/// rewrites the tail only when a prop spawns/despawns. Only `wow_model.wgsl` declares the tail
/// region — the other shaders mirror the PREFIX and bind the same (larger) buffer, which wgpu
/// allows. Keeping the probes out of this struct keeps it stack-cheap: the ExtractResource clone
/// runs every frame, and a ~900 KB by-value blob overflowed a render-thread stack (measured live).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct LightStd430 {
    rows: [[f32; 4]; LIGHT_HEADER_ROWS],
    points: [[f32; 4]; 2 * MAX_POINT_LIGHTS],
}

/// Header row count of the canonical layout above (rows 0..=20). Every producer of a light blob
/// sizes against this — the portrait booth included.
pub const LIGHT_HEADER_ROWS: usize = 21;

/// Pack the **model-lighting core** into `rows` — every row the model shaders' lit lanes derive
/// from the (ambient, diffuse, sun_dir) triple: rows 0-2 (ambient/diffuse/sun, w = enables), the
/// SH block (rows 6-11 + row 12 `.xyz`; DC = ambient in the `.w` lanes), and the sun's SH DC
/// redistribution (row 17 `.yzw`, at intensity 1). Leaves every other row — and row 12 `.w`
/// (free) / row 17 `.x` (SIDN) — untouched.
///
/// The sun's bands are the `Model2.bls` closed form — the SAME [`sh::prop_probe_coeffs`] fold the
/// interior lane runs, read off the shipped ARB vertex program: `E(n) = D·(3 + 16μ + 15μ²)/34`, μ =
/// n·u toward-light, EVERY band linear in the committed colour `D` — there is no separate amplitude
/// scalar, and the per-instance intensity lives entirely in that colour (a consumer multiplies ALL
/// sun terms by I; packed here at I = 1). The peak (μ=1) equals the FFP walls' `D·(N·L)`
/// peak by construction (the 16/17 accumulate scale exists for exactly that), and the closed form
/// never goes meaningfully negative — the old trace-fit's ~¼-strength lobe with a negative back
/// side (shadow-side characters turned blue as the warm channels floored at 0) is superseded.
///
/// **The SH block (rows 6-11, row 12 `.xyz`, row 17 `.yzw`) is the live exterior M2 response**
/// (0803). It was dormant for months — 0410 took the lane off this curve onto a hard-cutoff FFP
/// matte on the director's look call and nothing consumed the rows — until 0796 refuted the fidelity
/// premise behind that retirement (the reference's M2 lane IS this SH shader) and 0799 put the two
/// side by side for the call. Anything that stops writing these rows now renders every exterior
/// doodad and creature black. The interior-prop and glue-rig lanes are unaffected — they fold their
/// own probes through the per-instance `prop_probes` table, not these rows.
///
/// This is the ONE packer for the scene light ([`build_light_data`]) AND the portrait booth's
/// studio light (`portrait::setup_booths`): the booth used to hand-copy the layout and rendered
/// black portraits the day 0354 moved the lit lanes onto rows it never wrote. A producer that
/// copies the layout goes stale the day the layout moves — so producers don't copy it, they call
/// this.
pub fn pack_model_core_rows(
    rows: &mut [[f32; 4]; LIGHT_HEADER_ROWS],
    ambient: [f32; 3],
    diffuse: [f32; 3],
    sun_dir: Vec3,
) {
    rows[0] = [ambient[0], ambient[1], ambient[2], 1.0]; // 0 light_ambient (w=Mod2x 1.0)
    rows[1] = [diffuse[0], diffuse[1], diffuse[2], 1.0]; // 1 light_diffuse (w=clamp on)
    rows[2] = [sun_dir.x, sun_dir.y, sun_dir.z, 1.0]; // 2 light_sun (w=dir/SH enable 1.0)
                                                      // The sun lobe folded at intensity 1 with NO ambient — ambient rides the DC lanes directly
                                                      // (it never scales with the per-instance intensity), while the fold's own `.w` output is the
                                                      // sun's DC redistribution, re-homed onto row 17 `.yzw` so the shader can scale it by I.
    let sun = sh::prop_probe_coeffs([0.0; 3], &[(-sun_dir, diffuse)]);
    for (i, row) in sun.iter().enumerate().take(6) {
        rows[6 + i] = row.to_array(); // 6-8 sh_c10_{r,g,b} · 9-11 sh_c13_{r,g,b}
    }
    rows[6][3] = ambient[0]; // the DC lanes carry ambient alone
    rows[7][3] = ambient[1];
    rows[8][3] = ambient[2];
    rows[12][0] = sun[6].x; // 12 sh_c16 xyz — .w is a free lane (see the struct comment)
    rows[12][1] = sun[6].y;
    rows[12][2] = sun[6].z;
    // 17 `.yzw` — the sun's SH DC redistribution at intensity 1 (`D·(4/17)(0.375+0.9375(uₓ²+u_y²))`
    // per channel): an SH consumer adds it × the per-instance intensity (dormant since 0410 — see
    // the doc above). `.x` (SIDN) is the scene's.
    rows[17][1] = sun[0].w;
    rows[17][2] = sun[1].w;
    rows[17][3] = sun[2].w;
}

/// The reference's committed point-light diffuse is the **RAW** `colour × intensity × modelFade`
/// — over-gamut values included (`0x71ca80` → `0x593040`; compose arithmetic, animate leg
/// `716a67`–`716aa6`).
///
/// `0x71ca80` — which two prior rounds read as a clamp01 and then as a peak-normalize — is
/// actually a lossy **RGBE-style encoder**: it stores a peak-normalized byte colour at
/// `CGxLight+0x14` *and* the raw peak float `m = max(1, r, g, b)` at `+0x20`, and the device copy
/// `0x593040` **decodes them right back** (`byte · m/255 ≈ raw channel`) before the GL light is
/// set. Net effect: identity up to 8-bit peak-relative quantization (≤ ~0.5 %, which we skip). A
/// night terrain draw in the ring capture commits `(1.2, 1.035, 0.805)` verbatim — over-white
/// preserved. So we pack the raw product; the saturation the eye sees comes from the *vertex*
/// clamp of the summed lighting (GL T&L clamps `ambient + sun + Σ points` per vertex BEFORE
/// interpolation — see `terrain.wgsl`), never from the commit.
pub fn commit_raw(rgb: [f32; 3]) -> [f32; 3] {
    rgb.map(|c| c.max(0.0))
}

/// Capacity of the packed point-light table (fixed-size in the WGSL mirror structs — keep in sync).
/// 256 lights × 2 rows × 16 B = 8 KB — generous for the densest streamed village/city interior set;
/// [`build_light_data`] packs the nearest-to-camera first when over capacity.
pub(super) const MAX_POINT_LIGHTS: usize = 256;

/// Pack lights only within this camera distance (yd). A point light's whole visible effect lives
/// within its ~48 yd candidacy range (`spawn::POINT_LIGHT_RANGE`, the packed `.w`); a pool farther
/// than ~300 yd is sub-pixel and usually fogged, and the cap keeps the per-vertex selection walk
/// (0285: each unit picks its ≤3 nearest from this table) bounded.
const POINT_PACK_RADIUS: f32 = 300.0;

/// **The rooms a point light belongs to** — a WMO's own MOLT fixture (the groups whose MOLR names
/// it) or one of its props' M2 lights (the groups whose MODR names the prop). Absent on an ADT map
/// doodad's light, a creature's, a GameObject's: nothing claims those.
///
/// The fourth rider of decision 0689's law, after the prop's mesh, its particle clouds and its
/// ribbon trails. The reference never needs it: a WMO's furniture is instantiated out of each
/// **visible** group's MODR list, so a torch in a culled room does not exist and registers no
/// light. Its light-register walk really does have no visibility term of its own — the gate for a
/// model entering the register walk is the scene update-list activation flag `[model+0x10]`, "not
/// visibility, not distance, not LOD", and the ≤4 cap is purely receiver-side. So the faithful fix
/// is NOT a visibility test bolted onto the gather; it is that the SOURCE should not be there at
/// all, which is what this component says.
/// [`build_light_data`] drops the light while its rooms are culled, exactly as the model-visibility
/// authority drops the prop's own submeshes.
///
/// A newtype rather than a bare [`crate::wmo_portal::WmoGroupVis`] on purpose: that component on a
/// light entity would enlist it in `apply_model_visibility`'s `group_only` query — a `PointLight`
/// carries `Visibility` and `GlobalTransform`, so it matches — making the model-visibility
/// authority a second writer on an entity whose `Visibility` nothing reads (decision 0025).
#[derive(Component)]
pub struct LightRooms(pub(crate) crate::wmo_portal::WmoGroupVis);

/// **An authored WoW point light source** — an M2 light, a WMO MOLT omni, a carried torch —
/// as the packed light table reads it. This used to be Bevy's `PointLight`, kept purely as a
/// data carrier: [`build_light_data`] was its only reader in the engine, every lit surface
/// takes its lights from the shared table (0273/0285), and no shader here consumes Bevy's
/// clustered lights at all. Bevy nonetheless ran its whole light lane over every one of them
/// each frame — `assign_objects_to_clusters` (a `Vec` rebuilt per frame with a `RenderLayers`
/// clone per light, even with the world camera's `ClusterConfig::None`), `extract_lights`,
/// `prepare_lights`, the light visibility check — a city of lamps' worth of work for nothing,
/// ~2 % of the alone frame on the crowd rig's sampled profile (decision 1945). The fields keep
/// the `PointLight` numbers exactly (`intensity` in the same 4π-scaled units), so the packing
/// below and every recipe are unchanged.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct WorldPointLight {
    /// Linear RGB, hue preserved.
    pub color: [f32; 3],
    /// `4π × authored intensity` — the `PointLight` convention, so the packer's `/(4π)` reads
    /// the authored product back.
    pub intensity: f32,
    /// The ≤3-nearest selection-candidacy radius (yd) — see `terrain_stream::point_light`.
    pub range: f32,
}

/// Main-world resource holding the packed light for this frame; extracted into the render world where
/// [`upload_light`] writes it. Rebuilt every frame by [`build_light_data`] (cheap — one std430 pack).
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

/// The one persistent storage buffer all materials bind. Created once in [`create_shared_light_buffer`]
/// (main world, so material construction can clone it into the `#[storage(90, …)]` field), then cloned
/// into the render world via `ExtractResource` so [`upload_light`] can write it. `Buffer` clone shares
/// the same GPU resource.
#[derive(Resource, Clone, ExtractResource)]
pub struct SharedLightBuffer(pub Buffer);

/// Wire the shared-light infra into the app. `build_light_data` is chained after the lighting resolve
/// in [`super::LightingPlugin`]; this adds the resource, the extract plugins, the startup buffer
/// creation, and the render-world upload.
pub(super) fn register(app: &mut App) {
    app.init_resource::<WowLightData>()
        .init_resource::<super::prop_probes::PropProbeExtract>()
        .add_plugins(ExtractResourcePlugin::<WowLightData>::default())
        .add_plugins(ExtractResourcePlugin::<SharedLightBuffer>::default())
        .add_plugins(ExtractResourcePlugin::<super::prop_probes::PropProbeExtract>::default())
        // PostUpdate, **after transform propagation**: the point table is packed from each light's
        // `GlobalTransform`, and a CARRIED light (0587 — the torch in an NPC's hand) is a child of a
        // moving joint, so its global is only correct once `Propagate` has run. Packed from `Update`
        // it read the PREVIOUS frame's pose — the pool rubber-banded behind a walking bearer, and a
        // freshly spawned light packed one frame at the world origin. A world-baked doodad light
        // never moves, which is why this was invisible until entities started carrying lights.
        .add_systems(
            PostUpdate,
            build_light_data
                .after(bevy::transform::TransformSystems::Propagate)
                .after(super::update_time_lighting),
        )
        // After the spawners (PostUpdate): publish the probe table for extraction on change.
        .add_systems(PostUpdate, super::prop_probes::publish_prop_probes);
    // Guarded like every other render-side registration in the tree: a headless build (no GPU,
    // `backends: None`) has no render app, and the schedule tests build the engine that way.
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.add_systems(
            Render,
            (upload_light, super::prop_probes::upload_prop_probes)
                .in_set(RenderSystems::PrepareResources),
        );
    }
}

/// Create the single persistent storage buffer. `RenderDevice` is a main-world resource (inserted in
/// `RenderPlugin::finish`, available from `Startup` on), so the `assets` foundation builds it alongside
/// `WorldAssets` (which stores a clone so `model_material` can hand it to every model) and inserts the
/// returned resource (cloned into the render world for [`upload_light`]). `STORAGE | COPY_DST` (storage
/// binding + per-frame `write_buffer`).
pub fn new_shared_light_buffer(device: &RenderDevice) -> SharedLightBuffer {
    SharedLightBuffer(device.create_buffer(&BufferDescriptor {
        label: Some("wow_shared_light"),
        size: light_blob_bytes(),
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    }))
}

/// The full byte size of the shared light BUFFER: the per-frame blob ([`LightStd430`] — 19 header
/// rows + the point-light table) PLUS the interior-prop probe region PLUS the skin-palette
/// regions (rig slot table + tint table + rig-origin table + mat-anim table + straddle clip
/// table + palette rows — decisions 0720/0812/0974/1381/2188) at the tail. **Every buffer bound as
/// `wow_light` must be at least this big** — `wow_model.wgsl` declares the whole layout,
/// and wgpu validates bound size against the shader's struct at draw time. The portrait booth's
/// frozen studio-light buffer sizes itself with this (its table regions stay zeroed ⇒ no scene
/// point lights and black probes on portraits — the studio look is deliberately static); a
/// booth's PALETTE and ORIGIN regions are live, kept written by `rig_palette`'s mirror registry.
pub fn light_blob_bytes() -> u64 {
    per_frame_blob_bytes()
        + (7 * MAX_PROP_PROBES * 16) as u64
        + crate::rig_palette::palette_regions_bytes()
}

/// Byte size of the per-frame prefix alone (= the probe region's offset — see
/// `prop_probes::prop_probe_region_offset`).
pub(super) fn per_frame_blob_bytes() -> u64 {
    std::mem::size_of::<LightStd430>() as u64
}

/// Pack the resolved [`WowLighting`] (+ the global fog-disable toggle and the view farclip) into the
/// std430 blob. The `.w` lanes carry the faithful invariants the shaders expect (Mod2x 1.0, clamp on,
/// terrain shininess 20, fog-enable, farclip wall); the model SH coeffs and both water swatches are
/// derived here once per frame (they used to be recomputed + pushed per-material in `apply_wow_lighting`).
#[allow(clippy::type_complexity)]
fn build_light_data(
    light: Res<WowLighting>,
    debug: Res<DebugState>,
    view: Res<ViewDistance>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    lights_q: Query<(&WorldPointLight, &GlobalTransform, Option<&LightRooms>)>,
    // The per-frame portal PVS, for the room term below ([`LightRooms`]).
    portals: Query<&crate::wmo_portal::WmoPortalInstance>,
    mut data: ResMut<WowLightData>,
    time: Res<Time>,
    mut last_dump: Local<f64>,
    mut last_rows_dump: Local<f64>,
) {
    let l = &*light;
    let fog_enable = if debug.lighting.disable_fog { 0.0 } else { 1.0 };
    let farclip = view.farclip;
    // Per-kind water swatches (shallow/deep rgb + alpha). River/lake use the non-ocean path.
    let (rs, rd, rsa, rda) = l.water_colors(LiquidKind::Still);
    let (os, od, osa, oda) = l.water_colors(LiquidKind::Ocean);
    // Built in a scratch copy and written through `ResMut` only when a row moved: the extract
    // clones this 8.5 KB blob every frame it reads as changed, and a parked frame changes nothing.
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
                                           // 17 `.x` — the SIDN night fraction (the windows-glow-at-night ramp: `wow_model.wgsl`
                                           // multiplies each WMO SIDN material's authored emissive colour by it on the lit lanes).
                                           // `.yzw` is the core packer's below.
    rows[17][0] = l.sidn_night;
    // 18/19 — the INTERIOR fog triple (see the layout doc above). 19.zw are free lanes: they
    // carried retired dials (the 0273/0354-era A/Bs, the point gain, the 0750/0751 sun
    // calibration). 12.w was free too until 0796 gave it the response A/B (below).
    rows[18] = [
        l.wmo_fog_color[0],
        l.wmo_fog_color[1],
        l.wmo_fog_color[2],
        fog_enable,
    ];
    rows[19] = [l.wmo_fog_start, l.wmo_fog_end, 0.0, 0.0];
    // Rows 0-2, the SH block 6-12.xyz, and the sun DC (17.yzw) — the shared model-light core
    // (also the portrait booth's packer). Row 20 (point_count) is the point-table pack's below.
    pack_model_core_rows(rows, l.ambient, l.diffuse, l.sun_dir);
    // The dynamic point-light table (decision 0278): every spawned point light within
    // [`POINT_PACK_RADIUS`] of the camera, nearest-first when over capacity — the VERTEX stages of
    // `terrain.wgsl`/`wow_model.wgsl` walk it for the Gouraud point term (bevy's clusterable buffer
    // is fragment-only in the view layout, so the lights ride this buffer). Colour = the light's
    // effective linear RGB: bevy stores colour and intensity apart, and the spawn premultiplied 4π
    // (`spawn_point_light`), so `intensity/(4π)` recovers exactly the authored colour × intensity —
    // packed RAW as the reference commits it ([`commit_raw`]: the encode→decode round trip is
    // identity, over-gamut preserved). Entries past `count` stay stale in the blob — the count row
    // guards every reader.
    let cam_pos = cam.single().map(|t| t.translation()).unwrap_or(Vec3::ZERO);
    let mut pts: Vec<(f32, Vec3, f32, [f32; 3])> = lights_q
        .iter()
        .filter(|(_, _, rooms)| {
            // The ROOM term (decision 0689's law, fourth lane — see [`LightRooms`]). Not a
            // visibility test bolted onto a faithful gather: the reference's register walk has no
            // such term either, it simply never has a culled room's torch to register. Ungated for
            // every light that names no rooms, which is every light outside a building.
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
    // `WOW_POINTS_DUMP=1`: print the committed point table once a second — the numeric probe for
    // "what is actually lighting this ground". A pool that reads wrong is one of a small set of
    // measurable causes (a duplicate light stacking, a light at the wrong height, an over-driven
    // colour, a count that shouldn't be there), and every one of them is a number here. Throttled,
    // and capped at the nearest 8 so a torch-lit town doesn't flood the log.
    //
    // `WOW_POINTS_DUMP=frame` drops the throttle. A once-a-second dump can only answer "is the pool
    // right?", never "is it the *same* pool it was last frame?" — and B38's flicker turned out to
    // alternate frame to frame, which a 1 Hz sample cannot see at all. Reading a per-second dump as
    // evidence of per-frame stability is how that light was cleared once already (0665's parked
    // culling test made the same mistake with a different instrument).
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
            // How contested the three slots are for the chunk under the camera — the number that
            // decides whether ground pops as emitters move. Candidacy is the faithful Chebyshev
            // box (`terrain.wgsl`'s `TERRAIN_REACH`); the old 48-yd sphere is printed beside it so
            // the over-gather stays visible rather than being taken on trust.
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
    // `WOW_LIGHT_DUMP=frame` (or `=1` for 1 Hz): the WHOLE packed header, bit-exact, per frame.
    //
    // The point of dumping every row rather than the interesting ones is that B38 has now eliminated
    // every *per-material* and *per-instance* shading input by measurement — they are bit-identical
    // on bright and dim frames alike — which leaves this buffer and the view as the only things that
    // can still be moving. A dump of selected rows would answer "did ambient move?"; only the full
    // set answers "did ANY shading input move?", and that is the question worth a run. Rows are
    // printed as raw f32 bits, so a change far below a printed decimal cannot hide.
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

/// Render-world: write the packed light into the shared buffer in place, before any draw reads it
/// (`RenderSystems::PrepareResources`). One small upload per frame, independent of material count.
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

    /// GOLDEN — the **live exterior M2 response** (0803): `wow_model.wgsl`'s doodad/entity lane must
    /// reproduce `E = A + I·D·(4/17)(0.375 + 2μ + 1.875μ²)` off the rows [`pack_model_core_rows`]
    /// writes, with `A` NOT scaling by the per-instance intensity and every sun band scaling by it
    /// exactly once (never I²).
    ///
    /// This exists because the capture harness cannot check it. `visual.sh`-style captures are
    /// bit-deterministic on static scenes (water-noon: MAE 0.000) but NOT on the entity/GameObject
    /// scenarios this lane owns — measured run-to-run at MAE 5.1 (chest-shade-rear) and 8.8
    /// (creature-sun-rear), a noise floor far above the ~0.24 signal the response change produces
    /// (0799 §2). So the lane's correctness is pinned HERE, deterministically, and the captures are
    /// only good for "it compiles and it moves pixels".
    ///
    /// `eval_sh_lane` mirrors the WGSL lane-for-lane on purpose — read the two side by side; a
    /// channel or row swap in the shader is caught by eye against this, not by this test.
    #[test]
    fn the_sh_response_lane_matches_the_closed_form_at_every_intensity() {
        // Stormwind, minute ≈1185 — the bands were independently recovered from the reference's own
        // uploaded shader constants (0796 §1), so the test is anchored on a real committed pair.
        let ambient = [102.0 / 255.0, 97.0 / 255.0, 123.0 / 255.0];
        let diffuse = [255.0 / 255.0, 112.0 / 255.0, 0.0];
        let sun_dir = Vec3::new(0.31, -0.82, 0.48).normalize(); // travel dir; to-light = −this
        let mut rows = [[0.0f32; 4]; LIGHT_HEADER_ROWS];
        pack_model_core_rows(&mut rows, ambient, diffuse, sun_dir);

        /// The SH branch of `wow_model.wgsl`'s exterior doodad/entity lane, verbatim.
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
        // The peak is calibrated to the FFP peak by construction (the 16/17 accumulate scale) — so
        // moving onto this curve changed NOTHING on a surface square to the sun, and the whole
        // visible difference lives on the shadow side. That is why 0803 read subtle, not dramatic.
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
        // And the mid-back dip really is BELOW ambient — the low-order-SH ringing the reference
        // authors. Clamping the sun term per-term instead of the sum would erase it.
        let dip = eval_sh_lane(&rows, mid_back, 1.0);
        assert!(
            dip[0] < ambient[0],
            "mid-back should dip below ambient: {} vs {}",
            dip[0],
            ambient[0]
        );
    }

    /// GOLDEN — the **commit clamp** (`0x71ca80` with `w = 1.0` degenerates to clamp01), driven end
    /// to end through the real packer so removing the clamp from the pack expression fails here
    /// rather than in the director's eye.
    ///
    /// The held torch is the case that made it visible: authored `(0.467, 0.290, 0.133) × 3.0`, i.e.
    /// a red channel 40% past white. Unclamped it saturated the MCVT grid far wider than the
    /// reference and the ground pool read white instead of flame-orange.
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
        // The real authored torch light, through the real spawn recipe.
        app.world_mut().spawn((
            crate::terrain_stream::point_light([0.466_666_7, 0.290_196_1, 0.133_333_34], 3.0),
            GlobalTransform::from_translation(Vec3::new(0.0, 1.5, 0.0)),
        ));
        app.update();

        let rows = &app.world().resource::<WowLightData>().0;
        assert_eq!(rows.rows[20][0], 1.0, "the light packed");
        let rgb = rows.points[1];
        // The raw authored product — over-white preserved. Two earlier rounds "fixed" this to a
        // per-channel clamp and then a peak-normalize; the trace-confirmed mechanism is that the
        // `0x71ca80` encode is decoded straight back by `0x593040`, so the GL light receives the
        // raw `colour × intensity` (ring capture: a terrain draw commits (1.2, 1.035, 0.805)
        // verbatim). Saturation belongs to the receiving vertex's lighting clamp, not the commit.
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

    /// GOLDEN — the PACKER's SH block: evaluating the rows this packer writes (DC lane +
    /// grade.yzw × I + I × (linear + quad + x²−y²)) must reproduce the disassembled `Model2.bls`
    /// closed form `clamp01(ambient + D·I·(3 + 16μ + 15μ²)/34)` at every intensity rung (2.5 lit /
    /// 1.0 mid-band / 0.5 MCSH-shadowed). Pins the "every sun band scales by I, never I²" law and
    /// the row homes (ambient in the DC lanes, the sun's DC redistribution on grade.yzw).
    ///
    /// NB (0747): no shader currently READS rows 6-12.xyz / 17.yzw — the live exterior lane in
    /// `wow_model.wgsl` implements the same closed form inline (sun side only, `max(0, f(μ))`;
    /// the full block's back-side wrap stays out per the anti-sun ruling). This golden pins the
    /// packed block itself; the flagged cleanup is either wiring a lane to the rows or retiring
    /// the dead fold together with this test.
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
        // The whole back hemisphere stays non-negative BEFORE ambient — the retired trace-fit's
        // negative lobe (blue shadow-side characters) must never come back. Closed-form minimum is
        // −0.0373·C at μ≈−0.53; with ambient ≥ 0.038·D the sum never floors a channel at 0.
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
        // Sweep μ over the back hemisphere: the dip never exceeds the documented −0.0373·C·I.
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
