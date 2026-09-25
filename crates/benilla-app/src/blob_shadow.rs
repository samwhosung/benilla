//! The unit blob shadow: the dark oval under every player, NPC and creature, drawn by the
//! reference's `0x6d7920` and rebuilt here on the shared surface-decal projector.
//!
//! - Draw path: the per-frame model-node pass `0x683dd0` gates at `0x6d78f0` and draws through
//!   the selection ring's decal chain (`0x6d7330 → 0x6d6fa0 → 0x6d7480`). Its collector flags
//!   `0x2f0122` add liquid receivers to the ring's; `GroundDecalSurface` has none yet, so the
//!   shadow lands on terrain and WMO faces only.
//! - Frame slot: phase 1, among the opaque drains (`0x6812c5` in `0x681070`). Per unit the ring
//!   ticks first and the shadow draws second (`0x683ec3`), so the multiply darkens the ring. It
//!   lands after terrain and WMO and before footprints, M2 opaque, water and both M2 transparent
//!   passes, so every transparent paints over it: `Rung::SHADOW_SORT`.
//! - Texture: `Textures\ShadowBlob.blp`, a 32×32 grayscale radial blob under a binary alpha
//!   disc. The reference adds a 64×8 trapezoid ramp on a second stage (`0x6d81a0`/`0x6d82d0`)
//!   whose combine is untraced; here the ramp is a vertical vertex-alpha fade.
//! - Box (`0x711a20`, `0x6d7920`): the `playableAnimationLookup[0]` sequence's box (Stand, fixed
//!   per model, not the playing sequence), clamped into ±5 per axis pre-scale (a cap, never a
//!   floor), scaled, yaw-rotated and axis-aligned-bounded. Vertically `+(h/2)` up and
//!   `-(5/3)(h/2)` down about the origin. A zero horizontal box draws nothing.
//! - Appearance: multiply (`GL_DST_COLOR/GL_ZERO`, `EffectBlend::Multiply`), white vertices with
//!   alpha = the model's base alpha `CM2Model+0x180` (read at `0x6d7fd6`). That carries the
//!   appear and despawn fades, the first-person fade and the stealth or ghost aura fade
//!   (`0x60d180` drives the same `0x614f80`), but not the M2 colour and texture-weight tracks
//!   (`0x707aea`), exactly like `UnitRenderAlpha`. Unlit, no fog, no depth write; the texture is
//!   `Rgba8Unorm`, so the multiply runs on raw bytes as the reference's does.
//! - Registration: units, players and corpses cast one; game objects and dynamic objects never do
//!   (`0x670e94`, `0x613e10`). The reference's `shadowLOD` toggle is not built: always on.
//! - Deviation: the shadow is gated on `InheritedVisibility`, though the reference draws one under
//!   an undrawn body (`0x48161d`), because benilla's exterior-scene election hides units the
//!   reference never loads, and their shadows would stay on the floor below.
//!
//! Each shadow's triangles are rebuilt only when [`ShadowKey`] moves and copied onto the effect
//! stream every shown frame.

use benilla_assets::ModelAnimations;
use benilla_protocol::EntityKind;
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

use crate::creature_anim::AnimData;
use crate::net::{Embodied, NetEntity};
use benilla_world::decal::{DecalFrame, WorldDecal};
use benilla_world::particles::buffer::{begin_effect_frame, EffectVertex};
use benilla_world::schedule::WorldStage;
use benilla_world::view::WorldCamera;

/// The reference's shadow disc, created by `0x6d8070`.
const SHADOW_TEXTURE: &str = "mpq://textures/shadowblob.blp";
/// Each box corner component is clamped into ±5 yd pre-scale (`0x6992c0`, `0x699250`).
const BOX_CLAMP: f32 = 5.0;
/// The reference's degenerate-box epsilon (`[0x8029d4]`).
const DEGENERATE_EPS: f32 = 2.384e-7;

/// One unit's shadow record, a top-level entity despawned with its owner.
#[derive(Component)]
struct BlobShadow {
    owner: Entity,
}

/// Last frame's rebuild inputs. `surfaces` counts the decal receivers, so a tile streaming in
/// under a standing unit still re-arms the rebuild.
#[derive(Component, Default)]
struct ShadowKey {
    feet: Vec3,
    rotation: Quat,
    box_min: Vec3,
    box_max: Vec3,
    alpha: f32,
    surfaces: usize,
    shown: bool,
}

/// The cached world-space triangles; empty when hidden.
#[derive(Component, Default)]
struct ShadowVerts(Vec<EffectVertex>);

/// The shadow texture, kept so the census can report its load state: a missing texture
/// silently withholds every draw.
#[derive(Resource)]
struct ShadowAssets {
    texture: Handle<Image>,
}

pub(crate) struct BlobShadowPlugin;

impl Plugin for BlobShadowPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_shadow_assets)
            .add_systems(
                Update,
                (sync_shadows, update_shadows)
                    .chain()
                    // After net motion and input, so the decal follows this frame's transforms.
                    .after(WorldStage::Input),
            )
            // After the frame's stream clear.
            .add_systems(PostUpdate, push_shadows.after(begin_effect_frame));
    }
}

/// Load the shadow disc once.
fn setup_shadow_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    let texture = asset_server.load::<Image>(SHADOW_TEXTURE);
    commands.insert_resource(ShadowAssets { texture });
}

/// Keep one shadow record per player or unit whose animated model has built; despawn the ones
/// whose owner went. Corpses, which the reference also shadows (module docs), get none here.
#[allow(clippy::type_complexity)] // the filtered spawn-gate query, commented inline
fn sync_shadows(
    mut commands: Commands,
    // A mount child's `Transform` is parent-relative, so it casts none; the rider's shadow reads
    // the mount's box instead.
    units: Query<
        (Entity, &NetEntity),
        (
            With<ModelAnimations>,
            Without<crate::entities::mount::MountBody>,
        ),
    >,
    shadows: Query<(Entity, &BlobShadow)>,
    // The answer changes only when a model or a mount arrives or leaves; any other frame is
    // skipped.
    grew: Query<(), Added<ModelAnimations>>,
    mut shrank: RemovedComponents<ModelAnimations>,
    mounted: Query<(), Added<crate::entities::mount::MountBody>>,
    mut unmounted: RemovedComponents<crate::entities::mount::MountBody>,
) {
    if grew.is_empty()
        && mounted.is_empty()
        && shrank.read().next().is_none()
        && unmounted.read().next().is_none()
    {
        return;
    }
    let mut shadowed = EntityHashSet::default();
    for (entity, shadow) in &shadows {
        if units.get(shadow.owner).is_err() {
            commands.entity(entity).despawn();
        } else {
            shadowed.insert(shadow.owner);
        }
    }
    for (owner, net) in &units {
        if !matches!(net.kind, EntityKind::Player | EntityKind::Unit) || shadowed.contains(&owner) {
            continue;
        }
        commands.spawn((
            BlobShadow { owner },
            ShadowKey::default(),
            ShadowVerts::default(),
        ));
    }
}

/// Re-project each shadow whose inputs moved; clear it when the box degenerates, the fade
/// reaches zero, or no receiving surface is in the box (the reference's no-ground gate).
#[allow(clippy::type_complexity)]
fn update_shadows(
    time: Res<Time>,
    catalog: Option<Res<AnimData>>,
    shadow_assets: Option<Res<ShadowAssets>>,
    images: Res<Assets<Image>>,
    decals: WorldDecal,
    unit_alpha: benilla_world::model_fade::UnitRenderAlpha,
    owners: Query<
        (
            &Transform,
            &ModelAnimations,
            Has<Embodied>,
            Option<&crate::entities::mount::MountChild>,
            // The exterior-scene election's verdict on the root, after propagation.
            Option<&InheritedVisibility>,
        ),
        Without<BlobShadow>,
    >,
    // Mounted, the shadow reads the mount's Stand box at the mount's scale; which box the
    // reference uses for a mounted unit is untraced.
    mount_anims: Query<(&NetEntity, &ModelAnimations), With<crate::entities::mount::MountBody>>,
    mut shadows: Query<(&BlobShadow, &mut ShadowKey, &mut ShadowVerts)>,
    // A once-a-second census of shadows and why the hidden ones hid, at
    // `RUST_LOG=benilla_app::blob_shadow=debug` (a `benilla::` filter matches nothing).
    mut census_at: Local<f32>,
) {
    let now = time.elapsed_secs();
    let census = now >= *census_at;
    if census {
        *census_at = now + 1.0;
    }
    let (mut n_total, mut n_shown, mut n_no_owner, mut n_no_clip, mut n_degen, mut n_no_ground) =
        (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    // Apart from `n_no_owner`: an undrawn body still has an owner.
    let mut n_undrawn = 0u32;
    let surface_count = decals.receiver_count();
    for (shadow, mut key, mut verts) in &mut shadows {
        n_total += 1;
        let Ok((unit, anims, is_self, mount_child, drawn)) = owners.get(shadow.owner) else {
            // `sync_shadows` despawns it next frame.
            hide(&mut key, &mut verts);
            n_no_owner += 1;
            continue;
        };
        // The visibility deviation (module docs): an undrawn owner casts nothing.
        if !drawn.is_none_or(|v| v.get()) {
            hide(&mut key, &mut verts);
            n_undrawn += 1;
            continue;
        }
        // Until the mount's model lands, the rider's own box stands in.
        let (anims, extra_scale) = match mount_child.and_then(|mc| mount_anims.get(mc.0).ok()) {
            Some((mnet, manims)) => (manims, mnet.scale),
            None => (anims, 1.0),
        };
        // The box is `playableAnimationLookup[0]`'s sequence, Stand, fixed per model; `resolve(0)`
        // walks the same table, so a Stand-less model gets the reference's substitute.
        let stand = catalog.as_deref().map_or(0, |c| anims.resolve(0, &c.0).id);
        let clip = anims.find(stand);
        let Some(clip) = clip else {
            hide(&mut key, &mut verts);
            n_no_clip += 1;
            continue;
        };
        // Clamp into ±5 pre-scale, then scale.
        let s = (unit.scale.x * extra_scale).max(0.0);
        let bmin = clip
            .bounds_min
            .clamp(Vec3::splat(-BOX_CLAMP), Vec3::splat(BOX_CLAMP))
            * s;
        let bmax = clip
            .bounds_max
            .clamp(Vec3::splat(-BOX_CLAMP), Vec3::splat(BOX_CLAMP))
            * s;
        if bmax.x - bmin.x <= DEGENERATE_EPS || bmax.z - bmin.z <= DEGENERATE_EPS {
            // The reference's degenerate-box no-op exit (`0x61e9c0`): no shadow.
            hide(&mut key, &mut verts);
            n_degen += 1;
            continue;
        }
        // The unit's render alpha from the root, which also tells a pending appear fade (zero)
        // from a settled unit, and carries the first-person fade.
        let alpha = unit_alpha.get(shadow.owner);
        if alpha <= 0.0 {
            hide(&mut key, &mut verts);
            continue;
        }
        let next = ShadowKey {
            feet: unit.translation,
            rotation: unit.rotation,
            box_min: bmin,
            box_max: bmax,
            alpha,
            surfaces: surface_count,
            shown: true,
        };
        if key.shown && !key_changed(&key, &next) {
            n_shown += 1;
            continue;
        }
        // The four horizontal corners through the rotation with the vertical term zeroed, as
        // the reference's build does, then axis-aligned-bounded.
        let (mut min_x, mut max_x) = (f32::MAX, f32::MIN);
        let (mut min_z, mut max_z) = (f32::MAX, f32::MIN);
        for (x, z) in [
            (bmin.x, bmin.z),
            (bmin.x, bmax.z),
            (bmax.x, bmin.z),
            (bmax.x, bmax.z),
        ] {
            let w = unit.rotation * Vec3::new(x, 0.0, z);
            (min_x, max_x) = (min_x.min(w.x), max_x.max(w.x));
            (min_z, max_z) = (min_z.min(w.z), max_z.max(w.z));
        }
        // Vertically `+(h/2)` up, `-(5/3)(h/2)` down (`[0xcea60c]`, `[0xcea610]`).
        let half_v = (bmax.y - bmin.y) * 0.5;
        let frame = DecalFrame {
            center: unit.translation,
            sin: 0.0,
            cos: 1.0,
            min_x,
            max_x,
            min_z,
            max_z,
            min_y: -half_v * (5.0 / 3.0),
            max_y: half_v,
        };
        let span_v = frame.max_y - frame.min_y;
        verts.0.clear();
        let projected = span_v > 0.0
            && decals.project(
                &mut verts.0,
                &frame,
                |p| {
                    // The reference's second-stage ramp (`0x6d81a0`), run vertically: inferred
                    // from the asymmetric vertical reach, its combine being untraced.
                    alpha * shadow_ramp((p.y - frame.min_y) / span_v)
                },
                |x, z| frame.rect_uv(x, z),
            );
        *key = next;
        key.shown = projected;
        if projected {
            n_shown += 1;
        } else {
            verts.0.clear();
            n_no_ground += 1;
        }
        if census && is_self {
            // Where the own shadow's projection went: vertex y against feet y tells the surface
            // stood on from a receiver below.
            let (mut y_min, mut y_max) = (f32::MAX, f32::MIN);
            let (mut x_min, mut x_max) = (f32::MAX, f32::MIN);
            let (mut z_min, mut z_max) = (f32::MAX, f32::MIN);
            for v in &verts.0 {
                y_min = y_min.min(v.pos[1]);
                y_max = y_max.max(v.pos[1]);
                x_min = x_min.min(v.pos[0]);
                x_max = x_max.max(v.pos[0]);
                z_min = z_min.min(v.pos[2]);
                z_max = z_max.max(v.pos[2]);
            }
            // Emitted XZ extents against the frame rect: a mismatch indicts the projector.
            debug!(
                "self shadow world span: x [{:.3}, {:.3}] ({:.3}), z [{:.3}, {:.3}] ({:.3}); \
                 feet ({:.3}, {:.3}), yaw {:.1} deg",
                x_min,
                x_max,
                x_max - x_min,
                z_min,
                z_max,
                z_max - z_min,
                unit.translation.x,
                unit.translation.z,
                unit.rotation
                    .to_euler(bevy::math::EulerRot::YXZ)
                    .0
                    .to_degrees(),
            );
            debug!(
                "self shadow: feet y {:.3}, box y [{:.3}, {:.3}], {} verts, vert y [{:.3}, \
                 {:.3}]",
                unit.translation.y,
                unit.translation.y + frame.min_y,
                unit.translation.y + frame.max_y,
                verts.0.len(),
                y_min,
                y_max
            );
            // Per-vertex uvs: a span check cannot see a degenerate mapping.
            let uvs: Vec<String> = verts
                .0
                .iter()
                .map(|v| format!("({:.3},{:.3})", v.uv[0], v.uv[1]))
                .collect();
            debug!("self shadow uvs: {}", uvs.join(" "));
            // On the reference, HumanFemale's footprint is 0.77x0.74 yd, nearly centred.
            debug!(
                "self shadow box: bmin {:?} bmax {:?} rect x [{:.3}, {:.3}] z [{:.3}, {:.3}] \
                 (extent {:.3}x{:.3}, centre offset ({:.3}, {:.3}))",
                bmin,
                bmax,
                min_x,
                max_x,
                min_z,
                max_z,
                max_x - min_x,
                max_z - min_z,
                (min_x + max_x) * 0.5,
                (min_z + max_z) * 0.5,
            );
        }
    }
    if census && n_total > 0 {
        // The texture's content, not just its load: a white-decoded blob multiplies to nothing.
        let tex = shadow_assets.map_or("no-resource".into(), |a| {
            images.get(&a.texture).map_or("MISSING".into(), |img| {
                let (w, h) = (img.width(), img.height());
                let center = img
                    .data
                    .as_ref()
                    .and_then(|d| {
                        let i = ((h / 2) * w + w / 2) as usize * 4;
                        d.get(i..i + 4).map(|p| format!("{p:?}"))
                    })
                    .unwrap_or_else(|| "no-data".into());
                // The whole centre row: how much of the box the disc visibly fills.
                if let Some(d) = img.data.as_ref() {
                    let row: Vec<String> = (0..w as usize)
                        .map(|x| {
                            let i = ((h / 2) as usize * w as usize + x) * 4;
                            d.get(i..i + 4)
                                .map(|p| format!("{}/{}", p[0], p[3]))
                                .unwrap_or_default()
                        })
                        .collect();
                    debug!("blob row {}: {}", h / 2, row.join(" "));
                }
                format!("loaded {w}x{h} center {center} id {:?}", a.texture.id())
            })
        });
        debug!(
            "blob shadows: {n_total} ({n_shown} shown, {n_no_owner} ownerless, {n_undrawn} \
             undrawn, {n_no_clip} no-clip, {n_degen} degenerate, {n_no_ground} no-ground; \
             {surface_count} surfaces; texture {tex})"
        );
    }
}

/// Push every shown shadow onto the stream: one multiply draw per unit at the shadow rung, fog
/// off, the reference's shadow pass state.
fn push_shadows(
    assets: Option<Res<ShadowAssets>>,
    cam: Query<Entity, With<WorldCamera>>,
    mut draw: benilla_world::particles::buffer::WorldEffectDraw,
    shadows: Query<(Entity, &ShadowKey, &ShadowVerts)>,
) {
    let Some(assets) = assets else { return };
    let Ok(cam) = cam.single() else { return };
    for (entity, key, verts) in &shadows {
        if verts.0.is_empty() {
            continue;
        }
        // Unlit: the scene light is already in the ground it multiplies.
        let mut batch = draw
            .batch(cam, assets.texture.id())
            .multiply()
            .anchored(key.feet)
            .rung(
                benilla_world::sky_order::Rung::SHADOW_SORT,
                benilla_world::sky_order::Rung::DECAL_RASTER,
            )
            .owner(entity);
        batch.vertices(&verts.0);
        batch.tris();
    }
}

/// Clear the record so the next eligible frame rebuilds.
fn hide(key: &mut ShadowKey, verts: &mut ShadowVerts) {
    key.shown = false;
    verts.0.clear();
}

/// Whether an input moved beyond noise: a millimetre, ~0.05°, under one colour step.
fn key_changed(a: &ShadowKey, b: &ShadowKey) -> bool {
    const POS_EPS: f32 = 1e-3;
    a.feet.distance_squared(b.feet) > POS_EPS * POS_EPS
        || a.rotation.angle_between(b.rotation) > 1e-3
        || (a.box_min - b.box_min).abs().max_element() > POS_EPS
        || (a.box_max - b.box_max).abs().max_element() > POS_EPS
        || (a.alpha - b.alpha).abs() > 1.0 / 255.0
        || a.surfaces != b.surfaces
}

/// The reference's trapezoid alpha ramp (`0x6d81a0`/`0x6d82d0`): over `x = 12u`, `x/2` below 2,
/// 1 through 10, `(12-x)/2` after.
fn shadow_ramp(u: f32) -> f32 {
    let x = 12.0 * u.clamp(0.0, 1.0);
    if x < 2.0 {
        0.5 * x
    } else if x < 10.0 {
        1.0
    } else {
        (0.5 * (12.0 - x)).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rise to 1 at u=1/6, flat through u=5/6, fall to 0 at u=1.
    #[test]
    fn ramp_matches_reference_trapezoid() {
        assert_eq!(shadow_ramp(0.0), 0.0);
        assert!((shadow_ramp(1.0 / 12.0) - 0.5).abs() < 1e-6); // x=1 → 0.5
        assert!((shadow_ramp(1.0 / 6.0) - 1.0).abs() < 1e-6); // x=2 → 1.0
        assert_eq!(shadow_ramp(0.5), 1.0);
        assert!((shadow_ramp(5.0 / 6.0) - 1.0).abs() < 1e-6); // x=10 → 1.0
        assert!((shadow_ramp(11.0 / 12.0) - 0.5).abs() < 1e-6); // x=11 → 0.5
        assert_eq!(shadow_ramp(1.0), 0.0);
        // Out-of-range clamps, never negative.
        assert_eq!(shadow_ramp(-1.0), 0.0);
        assert_eq!(shadow_ramp(2.0), 0.0);
    }

    /// The ±5 clamp is a pre-scale cap, not a floor.
    #[test]
    fn box_clamp_caps_pre_scale() {
        let raw = Vec3::new(-7.0, 0.0, 3.0);
        let clamped = raw.clamp(Vec3::splat(-BOX_CLAMP), Vec3::splat(BOX_CLAMP));
        assert_eq!(clamped, Vec3::new(-5.0, 0.0, 3.0));
        // A scale-2 unit's clamped box still doubles.
        assert_eq!(clamped * 2.0, Vec3::new(-10.0, 0.0, 6.0));
    }
}
