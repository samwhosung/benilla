//! Ground-level spell-effect quads as **projected surface decals** — the third client of the
//! shared decal projector (`decal.rs`), after the selection ring and the unit blob shadow.
//!
//! A base-anchored effect model's flat ground-plane quads (Battle Shout's six crescents, the
//! paladin auras' rings, Consecration's and Flamestrike's hovering burn discs —
//! [`benilla_formats::GroundQuad`], the `groundscan` z=0 + hover populations) author every
//! vertex on one ground-level plane (z = 0, or hovering fractions of a yard above it) with depth-test ON: drawn as
//! free geometry they are buried per-pixel by the first up-slope and float over the first
//! down-slope. The real 1.12 client draws them exactly that way (standard M2 batch pipeline —
//! no ground conform exists in the spell-visual chain); this lane is a deliberate modern
//! improvement, director-directed: anything that *needs ground level* renders like the ring does.
//!
//! Mechanism: the game's fx attach (`benilla::entities`) spawns one [`GroundFxDecal`] entity per ground quad
//! of a base-anchored instance instead of a mesh child. Each frame ([`update_ground_fx_decals`],
//! in [`crate::billboard::BillboardPlace`] — post-propagation, like the cards), the quad's four
//! authored corners are posed through its live joint × the bone's inverse bindpose (exactly the
//! skinned-vertex path, so the authored slide/spin/scale animation is preserved); when the posed
//! corners moved (the ShadowKey treatment; a static pose costs a compare), a
//! projection frame is fitted to the posed rectangle and the ground triangles inside it are
//! re-emitted with the quad's own UVs bilerped across the frame — the crescent drapes the
//! terrain it crosses. The cached triangles are pushed onto the shared effect stream every
//! frame with the part's authored identity riding the draw record: its blend
//! ([`crate::particles::buffer::EffectBlend::from_model`]), its `0x70baf0` fog policy, its
//! M2Color RGB loop and `MatAnim` alpha loop sampled into the vertex tint at push time (the old
//! path's per-instance material clones and their per-frame mutations are gone). A decal
//! despawns when its joint does (the effect instance's reap/self-termination), the billboard
//! cards' orphan rule; it pushes nothing when no receiving surface is in the box (mid-air), the
//! ring's no-ground gate.

use avian3d::prelude::Collider;
use benilla_assets::coords::wow_to_bevy;
use benilla_formats::GroundQuad;
use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::collision::GroundDecalSurface;
use crate::decal::{project_decal, DecalFrame};
use crate::particles::buffer::{EffectBlend, EffectDrawSpec, EffectFog, EffectQuads, EffectVertex};
use crate::view::WorldCamera;

/// One ground-quad decal: the live effect-rig joint it rides, the authored quad it projects,
/// the part's draw identity, and the cached projection.
#[derive(Component)]
pub(crate) struct GroundFxDecal {
    /// The joint entity whose pose animates the quad (the effect model's own rig; the instance
    /// root for a boneless model). Its despawn — the instance's reap — despawns the decal.
    joint: Entity,
    /// The bone's inverse bindpose (identity for a boneless model): a corner's world position is
    /// `joint_global × ibp × corner`, exactly what the skinned mesh would compute.
    ibp: Mat4,
    /// The quad corners in Bevy model space (y = 0), normalized at spawn so the fitted frame's
    /// `+z'` axis matches the UV `t` axis (see [`spawn_ground_fx_decal`]).
    corners: [Vec3; 4],
    /// The authored UV at each corner, parallel to `corners`.
    uvs: [[f32; 2]; 4],
    /// The part's texture (the shared material's, resolved at spawn).
    texture: Handle<Image>,
    /// The part's authored blend, mapped onto the lane (`model_render.rs`'s law).
    blend: EffectBlend,
    /// The part's `0x70baf0` fog policy (from the shared material's baked marker bits).
    fog: EffectFog,
    /// The part's M2Color RGB loop on this instance's clock (loop, attach-time origin) —
    /// sampled into the vertex tint at push time (was: a per-instance material clone mutated
    /// per frame through `FxTintAnims`).
    rgb_anim: Option<(std::sync::Arc<benilla_formats::RgbAnim>, f32)>,
    /// The part's **static** M2Color tint ([`GroundQuad::tint`]) — the constant colour the mesh
    /// path draws through its vertex-colour bake, which this lane has no vertex buffer to carry.
    /// White for a batch that authors none, and white whenever [`Self::rgb_anim`] is `Some` (the
    /// bake clears one when it emits the other), so the two multiply without ever double-applying.
    tint: [f32; 3],
    /// The cached projection (world-space effect triangles, white × the vertical-fade alpha).
    cache: Vec<EffectVertex>,
    /// The posed corners the cache was projected from (NaN-seeded: the first pass always
    /// projects) + the receiving-surface count that re-arms a static pose when a tile streams.
    cached_corners: [Vec3; 4],
    cached_surfaces: usize,
    /// The cached frame's center — the draw's sort anchor.
    center: Vec3,
}

/// Spawn one ground-quad decal entity (a world-root record — no render components; the
/// projection rides the effect stream). The caller resolves the part's texture/blend/fog/tint
/// identity from its shared material; the caller also inserts the part's `MatAnim` rider (its
/// `current` is the push-time alpha).
pub fn spawn_ground_fx_decal(
    commands: &mut Commands,
    texture: Handle<Image>,
    // The authored batch's blend and its material's packed fog byte, *not* the lane enums they
    // map to: a caller that translates those has to know the mapping, and the mapping is this
    // lane's (`EffectBlend::from_model`, `EffectFog::from_model_policy`).
    blend: benilla_formats::ModelBlend,
    additive: bool,
    fog_policy_bits: u32,
    rgb_anim: Option<(std::sync::Arc<benilla_formats::RgbAnim>, f32)>,
    quad: &GroundQuad,
    joint: Entity,
    ibp: Mat4,
) -> Entity {
    let blend = EffectBlend::from_model(blend, additive);
    let fog = EffectFog::from_model_policy(fog_policy_bits);
    let mut corners = quad.corners.map(wow_to_bevy);
    let mut uvs = quad.uvs;
    // Normalize corner handedness once, at rest: the WoW→Bevy rotation may map the authored
    // (+x, +y) rect axes to a pair whose second axis lands on the fitted frame's −z'. Runtime
    // joint transforms are proper rotations × positive scales, so this relative handedness is
    // invariant — one row swap here keeps the bilerp `t` axis aligned with `+z'` forever.
    let ex = corners[1] - corners[0] + corners[3] - corners[2];
    let ez = corners[2] - corners[0] + corners[3] - corners[1];
    if ez.z * ex.x - ez.x * ex.z < 0.0 {
        corners.swap(0, 2);
        corners.swap(1, 3);
        uvs.swap(0, 2);
        uvs.swap(1, 3);
    }
    commands
        .spawn(GroundFxDecal {
            joint,
            ibp,
            corners,
            uvs,
            texture,
            blend,
            fog,
            rgb_anim,
            tint: quad.tint,
            cache: Vec::new(),
            cached_corners: [Vec3::splat(f32::NAN); 4],
            cached_surfaces: 0,
            center: Vec3::ZERO,
        })
        .id()
}

/// Fit a projection frame to the posed quad: center at the corner mean, the frame's `x'` axis
/// along the (horizontally projected) `c0→c1` edge, half-extents from the averaged edge lengths,
/// the ring's vertical slab (±2 × the larger half-extent, so a ledge catches a fading smear).
/// `None` for a degenerate pose — the scale-0 first animation frame, or an edge-on tilt.
fn fit_frame(corners: &[Vec3; 4]) -> Option<DecalFrame> {
    let center = (corners[0] + corners[1] + corners[2] + corners[3]) * 0.25;
    let ex = (corners[1] - corners[0] + corners[3] - corners[2]) * 0.5;
    let ez = (corners[2] - corners[0] + corners[3] - corners[1]) * 0.5;
    let exh = Vec2::new(ex.x, ex.z);
    let (half_x, half_z) = (exh.length() * 0.5, Vec2::new(ez.x, ez.z).length() * 0.5);
    if half_x < 1e-3 || half_z < 1e-3 {
        return None;
    }
    let d = exh / (half_x * 2.0);
    let vert = 2.0 * half_x.max(half_z);
    Some(DecalFrame {
        center,
        // `in_frame` computes `x' = dx·cos − dz·sin`: cos = d.x, sin = −d.y sends the posed
        // `c0→c1` edge direction onto the frame's +x' axis.
        sin: -d.y,
        cos: d.x,
        min_x: -half_x,
        max_x: half_x,
        min_z: -half_z,
        max_z: half_z,
        min_y: -vert,
        max_y: vert,
    })
}

/// Bilinear UV over the quad's authored corner UVs at normalized frame coordinates `(s, t)`.
fn bilerp_uv(uvs: &[[f32; 2]; 4], s: f32, t: f32) -> [f32; 2] {
    let lerp2 =
        |a: [f32; 2], b: [f32; 2], k: f32| [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k];
    lerp2(lerp2(uvs[0], uvs[1], s), lerp2(uvs[2], uvs[3], s), t)
}

/// Per-frame placement + push (in [`crate::billboard::BillboardPlace`] — post-propagation, so
/// the joint pose is THIS frame's; after `begin_effect_frame` — the push lands in this frame's
/// stream): pose each decal's corners through its joint; re-project only when they (or the
/// receiving surfaces) moved; push the cached triangles tinted with this frame's RGB/alpha loop
/// samples. Orphaned decals (joint despawned with its effect instance) despawn; a frame with no
/// receiving ground pushes nothing (the ring's no-ground gate).
pub(crate) fn update_ground_fx_decals(
    mut commands: Commands,
    time: Res<Time>,
    cam: Query<Entity, With<WorldCamera>>,
    surfaces: Query<&Collider, With<GroundDecalSurface>>,
    joints: Query<&GlobalTransform, Without<GroundFxDecal>>,
    mut quads: ResMut<EffectQuads>,
    mut decals: Query<(
        Entity,
        &mut GroundFxDecal,
        Option<&crate::doodad_anim::MatAnim>,
    )>,
) {
    let Ok(cam) = cam.single() else { return };
    let now = time.elapsed_secs();
    let mut surface_count = usize::MAX;
    for (entity, mut decal, mat_anim) in &mut decals {
        let Ok(joint) = joints.get(decal.joint) else {
            commands.entity(entity).despawn();
            continue;
        };
        if surface_count == usize::MAX {
            surface_count = surfaces.iter().count();
        }
        let pose = joint.affine() * Affine3A::from_mat4(decal.ibp);
        let corners = decal.corners.map(|c| pose.transform_point3(c));
        // The rebuild gate: the posed corners capture the whole pose effect, so a
        // static aura under a static rig costs this compare. (NaN-seeded corners make the
        // first pass always project.)
        if corners != decal.cached_corners || surface_count != decal.cached_surfaces {
            let decal = &mut *decal;
            decal.cached_corners = corners;
            decal.cached_surfaces = surface_count;
            decal.cache.clear();
            if let Some(frame) = fit_frame(&corners) {
                let vert = frame.max_y;
                decal.center = frame.center;
                project_decal(
                    &mut decal.cache,
                    &surfaces,
                    &frame,
                    // The ring's vertical trapezoid: full within half the slab, fading to 0 at
                    // its edge — a wall/ledge smear dims with height instead of a hard clip.
                    |p| ((vert - p.y.abs()) / (0.75 * vert)).clamp(0.0, 1.0),
                    |x, z| {
                        let s = (x - frame.min_x) / (frame.max_x - frame.min_x);
                        let t = (z - frame.min_z) / (frame.max_z - frame.min_z);
                        bilerp_uv(&decal.uvs, s, t)
                    },
                );
            }
        }
        if decal.cache.is_empty() {
            continue;
        }
        // This frame's tint: the part's M2Color colour — its STATIC constant (the vertex-colour
        // bake this lane has no vertex buffer to carry) × its RGB loop where it varies (instance
        // clock) — and the MatAnim alpha loop. Exactly what the vertex colours + material clone +
        // MeshTag carried on the mesh path. Only one of the two colour terms is ever non-white
        // (the bake emits one or the other), so the product is the authored colour, not a square.
        let loop_tint = decal
            .rgb_anim
            .as_ref()
            .map_or([1.0, 1.0, 1.0], |(anim, origin)| anim.sample(now - origin));
        let tint: [f32; 3] = std::array::from_fn(|i| decal.tint[i] * loop_tint[i]);
        let alpha = mat_anim.map_or(1.0, |m| m.current);
        let start = quads.begin();
        quads.verts.extend(decal.cache.iter().map(|v| EffectVertex {
            pos: v.pos,
            uv: v.uv,
            color: [tint[0], tint[1], tint[2], v.color[3] * alpha],
        }));
        quads.commit_tris(
            start,
            EffectDrawSpec {
                cam,
                texture: decal.texture.id(),
                blend: decal.blend,
                fog: decal.fog,
                // Spell ground-fx art is authored to burn at its own colour (the lane law in
                // `EffectBlend::from_model`); no lit ground quad observed in the corpus.
                lighting: super::particles::buffer::EffectLighting::None,
                anchor: decal.center,
                bias: crate::sky_order::Rung::GROUND_FX,
                // The projector's shared coplanarity margin — NOT `GROUND_FX as i32`, which is a
                // *sort* rung borrowed for a rasterizer field because the two numbers happened to
                // coincide. That is the conflation 1806 is about, and it left this lane at +8192,
                // which the projector's own bake residual exceeds by 1.30× (1817).
                raster_bias: crate::sky_order::Rung::DECAL_RASTER,
                raster_slope: 0.0,
                cam_relative: false,
                no_depth_test: false,
                main_entity: entity,
                light: None,
                clip: None,
            },
        );
    }
}

/// Wire the ground-fx decal lane in.
///
/// The placement pass rides `BillboardPlace` — post-propagation, so the effect rig's joints carry
/// THIS frame's pose — and after the stream clear, since it pushes its cached projection into this
/// frame's stream. Registered here rather than by the game: the ordering is a property of the
/// lane, and a caller that has to know it is a caller that can get it wrong.
pub fn plugin(app: &mut App) {
    app.add_systems(
        bevy::app::PostUpdate,
        update_ground_fx_decals
            .in_set(crate::billboard::BillboardPlace)
            .after(crate::particles::buffer::begin_effect_frame),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A yawed, scaled rect must fit a frame that reproduces its extents and maps corners to the
    /// bilerp's (s, t) corners.
    #[test]
    fn fitted_frame_matches_posed_rect() {
        // A 2×1 rect (half-extents 1.0 / 0.5) yawed 30° about Y, raised to y = 3.
        let yaw = 30_f32.to_radians();
        let rot = Quat::from_rotation_y(yaw);
        let base = [
            Vec3::new(-1.0, 0.0, -0.5),
            Vec3::new(1.0, 0.0, -0.5),
            Vec3::new(-1.0, 0.0, 0.5),
            Vec3::new(1.0, 0.0, 0.5),
        ];
        let corners = base.map(|c| rot * c + Vec3::new(2.0, 3.0, -4.0));
        let frame = fit_frame(&corners).expect("non-degenerate");
        assert!((frame.center - Vec3::new(2.0, 3.0, -4.0)).length() < 1e-5);
        assert!((frame.max_x - 1.0).abs() < 1e-5 && (frame.max_z - 0.5).abs() < 1e-5);
        // Corner c0 must land at the frame's (min_x, min_z); c3 at (max_x, max_z).
        let at = |p: Vec3| {
            let (dx, dz) = (p.x - frame.center.x, p.z - frame.center.z);
            (
                dx * frame.cos - dz * frame.sin,
                dz * frame.cos + dx * frame.sin,
            )
        };
        let (x0, z0) = at(corners[0]);
        let (x3, z3) = at(corners[3]);
        assert!((x0 - frame.min_x).abs() < 1e-5 && (z0 - frame.min_z).abs() < 1e-5);
        assert!((x3 - frame.max_x).abs() < 1e-5 && (z3 - frame.max_z).abs() < 1e-5);
        // The vertical slab is ±2 × the larger half-extent.
        assert!((frame.max_y - 2.0).abs() < 1e-5 && (frame.min_y + 2.0).abs() < 1e-5);
    }

    /// A zero-scale pose (the first frame of an outward-scaling ring) fits no frame.
    #[test]
    fn degenerate_pose_fits_nothing() {
        let p = Vec3::new(1.0, 2.0, 3.0);
        assert!(fit_frame(&[p, p, p, p]).is_none());
    }

    /// Corner UVs bilerp exactly at the corners and average at the middle.
    #[test]
    fn bilerp_hits_corners() {
        let uvs = [[0.0, 0.0], [0.25, 0.0], [0.0, 1.0], [0.25, 1.0]];
        assert_eq!(bilerp_uv(&uvs, 0.0, 0.0), [0.0, 0.0]);
        assert_eq!(bilerp_uv(&uvs, 1.0, 0.0), [0.25, 0.0]);
        assert_eq!(bilerp_uv(&uvs, 0.0, 1.0), [0.0, 1.0]);
        assert_eq!(bilerp_uv(&uvs, 1.0, 1.0), [0.25, 1.0]);
        assert_eq!(bilerp_uv(&uvs, 0.5, 0.5), [0.125, 0.5]);
    }
}
