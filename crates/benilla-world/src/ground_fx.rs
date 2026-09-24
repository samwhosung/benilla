//! Ground-level spell-effect quads ([`benilla_formats::GroundQuad`]: Battle Shout's crescents, the
//! paladin auras' rings, Consecration's burn disc) drawn as decals through the shared projector.
//!
//! Deviation: the 1.12 client draws these flat quads as ordinary depth-tested M2 batches, so an
//! up-slope buries them and a down-slope leaves them floating; here they drape the terrain like
//! the selection ring, because an effect meant for ground level should sit on it.

use avian3d::prelude::Collider;
use benilla_assets::coords::wow_to_bevy;
use benilla_formats::GroundQuad;
use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::collision::GroundDecalSurface;
use crate::decal::{project_decal, DecalFrame};
use crate::particles::buffer::{EffectBlend, EffectDrawSpec, EffectFog, EffectQuads, EffectVertex};
use crate::view::WorldCamera;

/// One ground-quad decal: the joint it rides, the authored quad, its draw identity and its cache.
#[derive(Component)]
pub(crate) struct GroundFxDecal {
    /// The joint whose pose animates the quad; its despawn despawns the decal.
    joint: Entity,
    /// The bone's inverse bindpose: a corner lands at `joint_global × ibp × corner`, as a skinned
    /// vertex does, so the authored slide, spin and scale survive.
    ibp: Mat4,
    /// Bevy model-space corners (y = 0), ordered at spawn so the frame's `+z'` is the UV `t` axis.
    corners: [Vec3; 4],
    uvs: [[f32; 2]; 4],
    texture: Handle<Image>,
    blend: EffectBlend,
    /// The part's `0x70baf0` fog policy (from the shared material's baked marker bits).
    fog: EffectFog,
    /// The part's M2Color RGB loop and its attach-time origin, sampled into the tint at push time.
    rgb_anim: Option<(std::sync::Arc<benilla_formats::RgbAnim>, f32)>,
    /// The static M2Color tint ([`GroundQuad::tint`]) the mesh path bakes into vertex colours;
    /// white whenever [`Self::rgb_anim`] is `Some`, so the two never double-apply.
    tint: [f32; 3],
    /// The cached projection (world-space effect triangles, white × the vertical-fade alpha).
    cache: Vec<EffectVertex>,
    /// The posed corners (NaN-seeded, so the first pass projects) and the receiving-surface count
    /// the cache was built from; a streamed tile changes the count and re-projects a static pose.
    cached_corners: [Vec3; 4],
    cached_surfaces: usize,
    center: Vec3,
}

/// Spawns one ground-quad decal; the caller inserts the part's `MatAnim`, its push-time alpha.
pub fn spawn_ground_fx_decal(
    commands: &mut Commands,
    texture: Handle<Image>,
    // The authored blend and packed fog bits, not the lane enums: the mapping is this lane's.
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
    // Fix handedness at rest: the WoW→Bevy rotation can put the second rect axis on `−z'`, and a
    // joint pose (rotation × positive scale) keeps handedness, so one swap holds for good.
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

/// Fits a frame to the posed quad: centered on the corner mean, `x'` along the horizontal `c0→c1`
/// edge, a slab of ±2 × the larger half-extent; `None` for a scale-0 or edge-on pose.
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
        // `in_frame` takes `x' = dx·cos − dz·sin`, so these put the posed `c0→c1` edge on `+x'`.
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

fn bilerp_uv(uvs: &[[f32; 2]; 4], s: f32, t: f32) -> [f32; 2] {
    let lerp2 =
        |a: [f32; 2], b: [f32; 2], k: f32| [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k];
    lerp2(lerp2(uvs[0], uvs[1], s), lerp2(uvs[2], uvs[3], s), t)
}

/// Re-projects a decal when its posed corners or the receiving surfaces change and pushes the
/// cached triangles with this frame's tint; a decal whose joint is gone despawns.
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
        // The posed corners capture the whole pose, so a static aura costs only this compare.
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
                    // Full alpha for `|y|` up to a quarter of the slab's half-height, fading to 0
                    // at its edge, so a ledge smear dims with height instead of clipping.
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
        // The static tint times the RGB loop (only one is ever non-white), and the `MatAnim` alpha.
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
                // Unlit: spell ground-fx art burns at its own colour; the corpus has no lit one.
                lighting: super::particles::buffer::EffectLighting::None,
                anchor: decal.center,
                bias: crate::sky_order::Rung::GROUND_FX,
                // The projector's coplanarity margin, not the `GROUND_FX` sort rung.
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

/// Runs the placement pass in `BillboardPlace`, after transform propagation so joints carry this
/// frame's pose, and after `begin_effect_frame` clears the stream it pushes into.
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

    #[test]
    fn degenerate_pose_fits_nothing() {
        let p = Vec3::new(1.0, 2.0, 3.0);
        assert!(fit_frame(&[p, p, p, p]).is_none());
    }

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
