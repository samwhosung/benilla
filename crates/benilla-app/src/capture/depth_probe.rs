//! `WOW_DEPTH`: reads back the depth that won a pixel in the opaque pass, and how far away it is.
//!
//! `WOW_DEPTH="<x>,<y>[;<x>,<y>…]"` logs, per frame, the raw reverse-Z value at each screenshot
//! pixel and the surface's distance both along the pixel's ray (comparable with `WOW_PICK`'s hit
//! distances) and to the camera plane. `WOW_DEPTH_AT=<secs>` (default 20) and
//! `WOW_DEPTH_COUNT=<n>` (default 1) shape the sampling like the screenshot burst and the ray pick.
//! Pixels are used as given: the depth texture is allocated in physical pixels.
//!
//! `WOW_DEPTH_QUADS=<bone>[,<bone>…]` (empty value = every quad emitter) samples a grid inside
//! each live particle quad's projected corners instead, and logs the fraction that survives the
//! depth test and how deep the occluder sits in front, in yards. Frames with no live quad are
//! skipped and not counted.
//!
//! MSAA must be off (`WOW_MSAA=off`): a multisampled depth texture cannot be copied and has no
//! single depth per pixel, so the probe refuses.

use bevy::core_pipeline::core_3d::graph::{Core3d, Node3d};
use bevy::ecs::query::QueryItem;
use bevy::prelude::*;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_graph::{
    NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner,
};
use bevy::render::render_resource::{
    Buffer, BufferDescriptor, BufferUsages, Extent3d, MapMode, Origin3d, TexelCopyBufferInfo,
    TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect, TextureUsages,
};
use bevy::render::renderer::{RenderContext, RenderDevice};
use bevy::render::view::{ExtractedView, ViewDepthTexture};
use bevy::render::{Render, RenderApp, RenderSystems};

use super::probes::ProbeClock;
use benilla_world::particles::ParticleEmitter;
use benilla_world::view::WorldCamera;

pub(crate) struct DepthProbePlugin;

impl Plugin for DepthProbePlugin {
    fn build(&self, app: &mut App) {
        let pixels = std::env::var("WOW_DEPTH")
            .ok()
            .map(|s| parse_pixels(&s))
            .unwrap_or_default();
        let quad_bones = parse_bones(std::env::var("WOW_DEPTH_QUADS").ok().as_deref());
        if pixels.is_empty() && quad_bones.is_none() {
            warn!(
                "depth: WOW_DEPTH wants \"<x>,<y>[;<x>,<y>…]\" screenshot pixels (or set \
                 WOW_DEPTH_QUADS) — inert"
            );
            return;
        }
        // Quad mode arms at once: frames without a live quad are skipped anyway.
        let at = std::env::var("WOW_DEPTH_AT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(if quad_bones.is_some() { 0.0 } else { 20.0 });
        let count = std::env::var("WOW_DEPTH_COUNT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1u32)
            .max(1);
        app.insert_resource(DepthWatch {
            pixels,
            at,
            count,
            armed: false,
        })
        .insert_resource(QuadWatch(quad_bones))
        .init_resource::<QuadProbes>()
        .add_systems(Update, arm)
        .add_systems(
            PostUpdate,
            collect_quads.after(benilla_world::billboard::BillboardPlace),
        )
        .add_plugins((
            ExtractResourcePlugin::<DepthWatch>::default(),
            ExtractResourcePlugin::<QuadProbes>::default(),
            ExtractComponentPlugin::<DepthProbeView>::default(),
        ));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            warn!("depth: no render app — inert");
            return;
        };
        render_app
            .init_resource::<DepthFramesRead>()
            .add_systems(
                Render,
                (
                    prepare_staging.in_set(RenderSystems::PrepareResources),
                    // The node encodes the copy in the graph; after submit, staging maps.
                    read_depth.after(RenderSystems::Render),
                ),
            )
            .add_render_graph_node::<ViewNodeRunner<DepthReadbackNode>>(Core3d, DepthReadbackLabel)
            // After the opaque pass (`Opaque3d` and `AlphaMask3d`), before the transmissive one.
            // The retained static pass draws before the opaque pass, so its walls are in the read.
            // `WOW_DEPTH_AFTER=1` copies after the transparent pass instead, to read the depth the
            // transparent pass itself wrote.
            .add_render_graph_edges(
                Core3d,
                if std::env::var_os("WOW_DEPTH_AFTER").is_some() {
                    (
                        Node3d::MainTransparentPass,
                        DepthReadbackLabel,
                        Node3d::EndMainPass,
                    )
                } else {
                    (
                        Node3d::MainOpaquePass,
                        DepthReadbackLabel,
                        Node3d::MainTransmissivePass,
                    )
                },
            );
    }
}

/// The pixels to read and the sampling window.
#[derive(Resource, Clone)]
struct DepthWatch {
    pixels: Vec<(u32, u32)>,
    at: f32,
    count: u32,
    armed: bool,
}

impl ExtractResource for DepthWatch {
    type Source = DepthWatch;
    fn extract_resource(source: &Self::Source) -> Self {
        source.clone()
    }
}

/// `$WOW_DEPTH_QUADS`'s bone scope: `None` = the mode is off, `Some([])` = every quad emitter.
#[derive(Resource)]
struct QuadWatch(Option<Vec<u16>>);

/// One live particle quad, carried to the render world in the space the depth buffer is read in.
#[derive(Clone, Copy)]
struct QuadProbe {
    bone: u16,
    /// Index within the emitter's quads, written oldest-first: the last was born this frame.
    index: u32,
    /// The four corners in physical pixels, in `expand_quads`' own vertex order.
    corners: [Vec2; 4],
    /// The corners' mid NDC depth; `dspread` is their spread, which the reference's plain
    /// billboard (`0x7b2a50`) keeps at zero.
    dquad: f32,
    dspread: f32,
    /// The quad centre's distance to the camera plane, yards.
    viewz: f32,
}

/// This frame's live quads, in `$WOW_DEPTH_QUADS` scope.
#[derive(Resource, Clone, Default)]
struct QuadProbes(Vec<QuadProbe>);

impl ExtractResource for QuadProbes {
    type Source = QuadProbes;
    fn extract_resource(source: &Self::Source) -> Self {
        source.clone()
    }
}

/// Project every in-scope emitter's live quads into pixels, once a frame, from the shared effect
/// stream `BillboardPlace` fills, so each quad is measured as drawn. Child-pool quads share the
/// emitter's draw records and count under its bone.
fn collect_quads(
    watch: Res<QuadWatch>,
    mut probes: ResMut<QuadProbes>,
    cam: Query<(&Camera, &GlobalTransform, &Projection), With<WorldCamera>>,
    emitters: Query<(Entity, &ParticleEmitter)>,
    quads: Res<benilla_world::particles::buffer::EffectQuads>,
) {
    let Some(bones) = watch.0.as_deref() else {
        return;
    };
    probes.0.clear();
    let (Ok((camera, cam_tf, projection)),) = (cam.single(),) else {
        return;
    };
    let Some(vp) = camera.physical_viewport_size() else {
        return;
    };
    // The frame's own matrices, the same pair `depthdump` projects with.
    let clip_from_world = projection.get_clip_from_view() * cam_tf.to_matrix().inverse();
    for (entity, emitter) in &emitters {
        if !bones.is_empty() && !bones.contains(&emitter.bone()) {
            continue;
        }
        // Every draw record this emitter committed this frame (its own pool + child pools).
        let ranges = quads
            .draws
            .iter()
            .filter(|d| d.main_entity == entity)
            .map(|d| d.range.clone());
        let pos: Vec<[f32; 3]> = ranges
            .flat_map(|r| quads.verts[r.start as usize..r.end as usize].iter())
            .map(|v| v.pos)
            .collect();
        for (index, quad) in pos.as_chunks::<4>().0.iter().enumerate() {
            let mut corners = [Vec2::ZERO; 4];
            let (mut dmin, mut dmax, mut center) = (f32::MAX, f32::MIN, Vec3::ZERO);
            let mut behind = false;
            for (i, v) in quad.iter().enumerate() {
                // Stream verts are world-space (the sort anchor rides the draw record).
                let world = Vec3::from(*v);
                center += world / 4.0;
                let clip = clip_from_world * world.extend(1.0);
                if clip.w <= 0.0 {
                    behind = true;
                    break;
                }
                let ndc = clip.truncate() / clip.w;
                corners[i] = Vec2::new(
                    (ndc.x + 1.0) * 0.5 * vp.x as f32,
                    (1.0 - ndc.y) * 0.5 * vp.y as f32,
                );
                dmin = dmin.min(ndc.z);
                dmax = dmax.max(ndc.z);
            }
            if behind {
                continue;
            }
            probes.0.push(QuadProbe {
                bone: emitter.bone(),
                index: index as u32,
                corners,
                dquad: (dmin + dmax) * 0.5,
                dspread: dmax - dmin,
                viewz: -(cam_tf.to_matrix().inverse() * center.extend(1.0)).z,
            });
        }
    }
}

/// `"60,61"` → the bone scope; an empty value means every quad emitter, an unset one `None`.
fn parse_bones(spec: Option<&str>) -> Option<Vec<u16>> {
    Some(
        spec?
            .split(',')
            .filter_map(|b| b.trim().parse().ok())
            .collect(),
    )
}

/// Marks the one view whose depth to read; the UI camera shares the world camera's depth texture.
#[derive(Component, Clone, Copy, ExtractComponent)]
struct DepthProbeView;

/// The staging buffer, kept across frames so a 24-frame burst allocates once.
#[derive(Resource)]
struct DepthStaging {
    buffer: Buffer,
    bytes_per_row: u32,
    height: u32,
}

/// Once the sampling window opens, opt the world camera's depth texture into `COPY_SRC` and mark
/// it; the live `Msaa` component being anything but off disables the probe.
fn arm(
    mut watch: ResMut<DepthWatch>,
    time: ProbeClock,
    mut cam: Query<(Entity, &mut Camera3d, &Msaa), With<WorldCamera>>,
    mut commands: Commands,
) {
    if watch.armed || time.elapsed_secs() < watch.at {
        return;
    }
    let Ok((entity, mut camera, msaa)) = cam.single_mut() else {
        return;
    };
    if *msaa != Msaa::Off {
        error!(
            "depth: MSAA is {msaa:?} — a multisampled depth texture cannot be copied, and there is \
             no single depth per pixel to report. Re-run with WOW_MSAA=off. Probe disabled."
        );
        watch.armed = true;
        watch.count = 0;
        return;
    }
    camera.depth_texture_usages =
        (TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC).into();
    commands.entity(entity).insert(DepthProbeView);
    info!(
        "depth: reading {} pixels for {} frames",
        watch.pixels.len(),
        watch.count
    );
    watch.armed = true;
}

/// Allocate the staging buffer to match the view's depth texture, before the graph runs.
fn prepare_staging(
    watch: Option<Res<DepthWatch>>,
    view: Query<&ViewDepthTexture, With<DepthProbeView>>,
    staging: Option<Res<DepthStaging>>,
    device: Res<RenderDevice>,
    mut commands: Commands,
) {
    let Some(watch) = watch else { return };
    if !watch.armed {
        return;
    }
    let Ok(depth) = view.single() else { return };
    let size = depth.texture.size();
    // `Depth32Float` is 4 bytes a texel; a buffer copy's row stride is 256-byte aligned.
    let bytes_per_row = RenderDevice::align_copy_bytes_per_row(size.width as usize * 4) as u32;
    if staging.is_some_and(|s| s.bytes_per_row == bytes_per_row && s.height == size.height) {
        return;
    }
    commands.insert_resource(DepthStaging {
        buffer: device.create_buffer(&BufferDescriptor {
            label: Some("wow_depth_readback"),
            size: u64::from(bytes_per_row) * u64::from(size.height),
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }),
        bytes_per_row,
        height: size.height,
    });
}

#[derive(RenderLabel, Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DepthReadbackLabel;

/// Copies the depth texture right after the opaque pass. Read at the end of the main pass instead,
/// it also holds depth the transparent pass wrote, from something that tracks the camera.
#[derive(Default)]
struct DepthReadbackNode;

impl ViewNode for DepthReadbackNode {
    type ViewQuery = (&'static ViewDepthTexture, &'static DepthProbeView);

    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        (depth, _): QueryItem<Self::ViewQuery>,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let (Some(watch), Some(staging)) = (
            world.get_resource::<DepthWatch>(),
            world.get_resource::<DepthStaging>(),
        ) else {
            return Ok(());
        };
        // `read_depth` counts the frames; stop copying once the burst is done.
        if !watch.armed || world.resource::<DepthFramesRead>().0 >= watch.count {
            return Ok(());
        }
        // Quad mode with nothing live this frame: no copy, and `read_depth` does not count it.
        if watch.pixels.is_empty()
            && world
                .get_resource::<QuadProbes>()
                .is_none_or(|q| q.0.is_empty())
        {
            return Ok(());
        }
        let size = depth.texture.size();
        // `COPY_SRC` lands only on the texture allocated after `arm`, so the first armed frame
        // still has the old one.
        if !depth.texture.usage().contains(TextureUsages::COPY_SRC) {
            return Ok(());
        }
        // Depth-stencil formats reject partial copies (wgpu-core `validate_texture_copy_range`), so
        // the whole texture goes across.
        render_context.command_encoder().copy_texture_to_buffer(
            TexelCopyTextureInfo {
                texture: &depth.texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::DepthOnly,
            },
            TexelCopyBufferInfo {
                buffer: &staging.buffer,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(staging.bytes_per_row),
                    rows_per_image: Some(size.height),
                },
            },
            Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }
}

/// How many frames [`read_depth`] has reported; a resource because the graph node reads it.
#[derive(Resource, Default)]
struct DepthFramesRead(u32);

/// Map back what the node copied and log the named pixels.
fn read_depth(
    watch: Option<Res<DepthWatch>>,
    quads: Option<Res<QuadProbes>>,
    view: Query<&ExtractedView, With<DepthProbeView>>,
    device: Res<RenderDevice>,
    staging: Option<Res<DepthStaging>>,
    depth: Query<&ViewDepthTexture, With<DepthProbeView>>,
    mut read: ResMut<DepthFramesRead>,
) {
    let (Some(watch), Some(staging)) = (watch, staging) else {
        return;
    };
    if !watch.armed || read.0 >= watch.count {
        return;
    }
    // Mirrors the node's quad-mode skip: nothing was copied, and the frame is not counted.
    let quads = quads.map(|q| q.0.clone()).unwrap_or_default();
    if watch.pixels.is_empty() && quads.is_empty() {
        return;
    }
    let (Ok(view), Ok(depth)) = (view.single(), depth.single()) else {
        return;
    };
    // Mirrors the node's own skip: no copy was encoded this frame, so there is nothing to map.
    if !depth.texture.usage().contains(TextureUsages::COPY_SRC) {
        return;
    }
    let size = depth.texture.size();
    let slice = staging.buffer.slice(..);
    slice.map_async(MapMode::Read, |_| {});
    // Block, so a frame's numbers never arrive under a later frame's index.
    if let Err(e) = device.poll(bevy::render::render_resource::PollType::wait_indefinitely()) {
        error!("depth: poll failed: {e}");
        return;
    }
    let frame = read.0;
    read.0 += 1;
    // The projection the frame was drawn with, once per burst: the distances derive from it.
    if frame == 0 {
        info!(
            "depth: {}x{} view, clip_from_view P₂₂ {} P₃₂ {} P₀₀ {} P₁₁ {}",
            size.width,
            size.height,
            view.clip_from_view.z_axis.z,
            view.clip_from_view.w_axis.z,
            view.clip_from_view.x_axis.x,
            view.clip_from_view.y_axis.y,
        );
    }
    let view_from_clip = view.clip_from_view.inverse();
    {
        let data = slice.get_mapped_range();
        for &(x, y) in &watch.pixels {
            if x >= size.width || y >= size.height {
                warn!(
                    "depth#{frame} ({x}, {y}): outside the {}x{} view",
                    size.width, size.height
                );
                continue;
            }
            let at = (y * staging.bytes_per_row + x * 4) as usize;
            let d = f32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
            match view_point(&view_from_clip, ndc_of(x, y, size.width, size.height), d) {
                // Along the ray (what `WOW_PICK` reports) and to the camera plane (what depth
                // encodes); they differ by 15% at the frame edge.
                Some(p) => info!(
                    "depth#{frame} ({x}, {y}): {d:.9}  =  {:.4} yd along the ray  ({:.4} yd view z)",
                    p.length(),
                    -p.z
                ),
                // Reverse-Z clears to 0 = infinitely far: nothing drew here at all.
                None => info!("depth#{frame} ({x}, {y}): {d:.9}  =  nothing drew (cleared)"),
            }
        }
        for q in &quads {
            report_quad(frame, q, &data, &staging, size.width, size.height);
        }
    }
    staging.buffer.unmap();
}

/// Samples per side across a quad's own area: 16x16, stable to under a percent.
const QUAD_GRID: usize = 16;

/// Run one quad's depth contest and log it. Reverse-Z with Bevy's default `GreaterEqual`: a
/// fragment survives iff `dquad >= dbuffer`. Samples interpolate the four projected corners, so
/// each lies inside the quad even when it is spun.
fn report_quad(
    frame: u32,
    q: &QuadProbe,
    data: &[u8],
    staging: &DepthStaging,
    width: u32,
    height: u32,
) {
    let (mut passed, mut total) = (0usize, 0usize);
    // Reverse-Z: `dmax` is the nearest occluder, `dmin` the furthest.
    let (mut dmin, mut dmax) = (f32::MAX, f32::MIN);
    let mut cleared = 0usize;
    for iy in 0..QUAD_GRID {
        for ix in 0..QUAD_GRID {
            let u = (ix as f32 + 0.5) / QUAD_GRID as f32;
            let v = (iy as f32 + 0.5) / QUAD_GRID as f32;
            let p = q.corners[0]
                .lerp(q.corners[1], u)
                .lerp(q.corners[3].lerp(q.corners[2], u), v);
            let (x, y) = (p.x.floor(), p.y.floor());
            if x < 0.0 || y < 0.0 || x >= width as f32 || y >= height as f32 {
                continue;
            }
            let at = (y as u32 * staging.bytes_per_row + x as u32 * 4) as usize;
            let d = f32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
            total += 1;
            if q.dquad >= d {
                passed += 1;
            }
            if d <= 0.0 {
                cleared += 1;
            } else {
                dmin = dmin.min(d);
                dmax = dmax.max(d);
            }
        }
    }
    if total == 0 {
        info!(
            "depth#{frame} quad bone={} i={}: entirely off screen",
            q.bone, q.index
        );
        return;
    }
    // On the reverse-Z curve view z scales as 1/d, so the ratio to `dquad` converts to yards.
    let yd = |d: f32| q.viewz * q.dquad / d;
    let (near, far) = (yd(dmax), yd(dmin));
    info!(
        "depth#{frame} quad bone={} i={} px=({:.0},{:.0}) dquad={:.9} spread={:.9} \
         viewz={:.4} pass={:.1}% ({passed}/{total}) occluder {near:.4}..{far:.4} yd \
         burial {:.4}..{:.4} yd cleared={cleared}",
        q.bone,
        q.index,
        q.corners[0].lerp(q.corners[2], 0.5).x,
        q.corners[0].lerp(q.corners[2], 0.5).y,
        q.dquad,
        q.dspread,
        q.viewz,
        passed as f32 / total as f32 * 100.0,
        q.viewz - far,
        q.viewz - near,
    );
}

/// A physical pixel's centre in NDC. Framebuffer rows run down, NDC y runs up.
fn ndc_of(x: u32, y: u32, width: u32, height: u32) -> Vec2 {
    Vec2::new(
        (x as f32 + 0.5) / width as f32 * 2.0 - 1.0,
        1.0 - (y as f32 + 0.5) / height as f32 * 2.0,
    )
}

/// Unproject a pixel's depth through the live matrix to its view-space point, in yards. The whole
/// point is needed: off axis, the ray length exceeds the plane distance by `1/cos θ`.
fn view_point(view_from_clip: &Mat4, ndc: Vec2, d: f32) -> Option<Vec3> {
    let p = *view_from_clip * Vec4::new(ndc.x, ndc.y, d, 1.0);
    // Behind the camera or at infinity (a reverse-Z clear reads 0 ⇒ w 0) means nothing drew here.
    (p.w.abs() > f32::MIN_POSITIVE)
        .then(|| p.truncate() / p.w)
        .filter(|v| v.is_finite() && v.z < 0.0)
}

/// `"x,y;x,y"` → pixels; a malformed pair is dropped with a warning.
fn parse_pixels(spec: &str) -> Vec<(u32, u32)> {
    spec.split(';')
        .filter(|s| !s.trim().is_empty())
        .filter_map(|pair| {
            let (x, y) = pair.split_once(',')?;
            match (x.trim().parse().ok(), y.trim().parse().ok()) {
                (Some(x), Some(y)) => Some((x, y)),
                _ => {
                    warn!("depth: skipping malformed pixel {pair:?}");
                    None
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_pixel_list_and_skips_junk() {
        assert_eq!(
            parse_pixels("412,396; 565,445 ;;nope;1,"),
            vec![(412, 396), (565, 445)]
        );
    }

    /// The projection the world camera actually draws with: Bevy's reverse-Z, infinite far.
    fn proj() -> Mat4 {
        Mat4::perspective_infinite_reverse_rh(
            std::f32::consts::FRAC_PI_4,
            3200.0 / 1800.0,
            benilla_world::view::NEARCLIP_DEFAULT,
        )
    }

    /// What the rasteriser writes for a point `dist` yards straight ahead down the view axis.
    fn depth_of(dist: f32) -> f32 {
        let clip = proj() * Vec4::new(0.0, 0.0, -dist, 1.0);
        clip.z / clip.w
    }

    #[test]
    fn a_depth_unprojects_to_the_distance_it_came_from() {
        let inv = proj().inverse();
        for dist in [2.0f32, 22.0, 46.0253, 46.0897, 3000.0] {
            let p =
                view_point(&inv, Vec2::ZERO, depth_of(dist)).expect("a drawn pixel has a point");
            assert!(
                (p.length() - dist).abs() < dist * 1e-3,
                "{dist} yd -> depth {} -> {} yd",
                depth_of(dist),
                p.length()
            );
        }
    }

    #[test]
    fn off_axis_the_ray_is_longer_than_the_perpendicular_distance() {
        // Straight ahead the two agree; at the frame edge they must not.
        let inv = proj().inverse();
        let d = depth_of(46.0);
        let centre = view_point(&inv, Vec2::ZERO, d).unwrap();
        assert!(
            (centre.length() - (-centre.z)).abs() < 1e-3,
            "on-axis they agree"
        );
        let edge = view_point(&inv, Vec2::new(-0.78, 0.42), d).unwrap();
        assert!(
            (-edge.z - 46.0).abs() < 0.05,
            "the perpendicular distance is what depth encodes: {} yd",
            -edge.z
        );
        assert!(
            edge.length() > 46.0 * 1.1,
            "off-axis the ray must be materially longer, got {} yd",
            edge.length()
        );
    }

    #[test]
    fn a_cleared_pixel_has_no_position() {
        // Reverse-Z clears to 0.0: infinitely far, i.e. nothing drew.
        assert_eq!(view_point(&proj().inverse(), Vec2::ZERO, 0.0), None);
    }

    #[test]
    fn the_awning_and_the_plank_are_thousands_of_ulps_apart() {
        // Two measured surfaces 1.4 cm apart perpendicular: the readback names the winner only if
        // that gap survives `f32`.
        let (awning, plank) = (46.0253f32, 46.0897f32);
        let (da, dp) = (depth_of(awning), depth_of(plank));
        let ulps = ((da.to_bits() as i64) - (dp.to_bits() as i64)).abs();
        assert!(
            ulps > 1000,
            "only {ulps} ULPs apart — a readback could not tell them apart"
        );
        // And they must not round to the same reported distance either.
        let inv = proj().inverse();
        let (ba, bp) = (
            view_point(&inv, Vec2::ZERO, da).unwrap().length(),
            view_point(&inv, Vec2::ZERO, dp).unwrap().length(),
        );
        assert!(
            (ba - bp).abs() > 0.01,
            "{ba} yd vs {bp} yd is not a distinguishable pair"
        );
    }
}
