//! The one model `Visibility` authority: dev toggles, the far-clip wall, the distance fade, the
//! portal PVS and the exterior window gate compose in one system. The first-person self-avatar
//! hide runs after [`super::ModelVisSet`] and wins on the self body.

use bevy::camera::primitives::Aabb;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use super::{blend_index, kind_index, ModelKind, ModelPart};
use crate::dev_state::DebugState;
use crate::model_fade::{doodad_fade_alpha, DoodadFade};
use crate::view::ViewDistance;
use crate::view::WorldCamera;
use crate::wmo_portal::{WmoGroupVis, WmoPortalInstance};
use benilla_assets::materials::WowModelMaterial;

/// Shows or hides each model submesh. A doodad or WMO part is culled once its bound lies wholly
/// past the far-clip wall ([`crate::view::within_farclip`]), so a straddler dissolves through the
/// per-pixel wall. Writes only on a flip.
#[allow(clippy::type_complexity)]
pub(super) fn apply_model_visibility(
    debug: Res<DebugState>,
    view: Res<ViewDistance>,
    cam: Query<
        (
            Ref<GlobalTransform>,
            Ref<Projection>,
            Option<Ref<Transform>>,
        ),
        With<WorldCamera>,
    >,
    // Scene edges the per-part skip cannot read off its own row: a portal set changing (a
    // building's asset lands frames after its spawn) and any owner's inherited verdict flipping.
    new_portals: Query<(), Changed<WmoPortalInstance>>,
    owner_flips: Query<(), Changed<InheritedVisibility>>,
    // A far-side mark the classifier strips under a still camera must un-skip its part.
    mut unmarked: RemovedComponents<super::FarSideOfWater>,
    instances: Query<&WmoPortalInstance>,
    windows: Res<crate::wmo_portal::ExteriorWindows>,
    // The camera's own building is not exterior scene: the reference draws it through the
    // interior portal walk, never the per-window exterior populate (`[0xc7b748]` is the branch).
    claim: Res<crate::wmo_portal::CameraInteriorClaim>,
    // Read-only: this system owns the `DoodadFade` handle pick, and the far side composes into it.
    far_twins: Res<super::FarSideTwins>,
    // Billboard card owners (a model root or a rig joint): a card is a world root, so nothing
    // else carries its owner's hide to it.
    card_owners: Query<&InheritedVisibility>,
    mut q: Query<(
        Entity,
        &ModelPart,
        Ref<GlobalTransform>,
        &mut Visibility,
        Option<&DoodadFade>,
        Option<&mut MeshTag>,
        Option<&mut MeshMaterial3d<WowModelMaterial>>,
        Option<&Aabb>,
        Option<Ref<WmoGroupVis>>,
        Option<Ref<crate::doodad_anim::MatAnim>>,
        Has<crate::exterior_cull::ExteriorScene>,
        Option<Ref<super::FarSideOfWater>>,
        Option<&crate::billboard::BillboardCard>,
    )>,
    // A building's MLIQ surfaces: a `WmoGroupVis` but no `ModelPart`, so none of the submesh
    // rules. A query here rather than a second writer, so a culled room's water goes with it.
    mut group_only: Query<
        (
            &WmoGroupVis,
            &mut Visibility,
            &GlobalTransform,
            Option<&Aabb>,
            Has<crate::exterior_cull::ExteriorScene>,
            Option<&mut MeshTag>,
        ),
        Without<ModelPart>,
    >,
) {
    let m = &debug.models;
    let cam_view = cam.iter().next();
    let cam_t = cam_view.as_ref().map(|(t, _, _)| t);
    // Every whole-scene input still since last frame: a part whose own row is still too would
    // come out unchanged, so it skips the walk. The camera's local `Transform` counts too: this
    // runs before propagation, so a teleport's move shows only there.
    let scene_still = cam_view.as_ref().is_some_and(|(t, p, l)| {
        !t.is_changed() && !p.is_changed() && !l.as_ref().is_some_and(|l| l.is_changed())
    }) && !debug.is_changed()
        && !view.is_changed()
        && !windows.is_changed()
        && !claim.is_changed()
        && !far_twins.is_changed()
        && new_portals.is_empty()
        && owner_flips.is_empty();
    let cam_pos = cam_t.map(|t| t.translation());
    let cam_fwd = cam_t.map(|t| Vec3::from(t.forward()));
    let gate = crate::exterior_cull::ExteriorGate::build(
        &windows,
        cam_view.as_ref().map(|(t, p, _)| (&**t, &**p)),
    );
    // The placement the camera stands in, exempt from its own window gate.
    let own_instance = claim.0.map(|c| c.room.instance);
    // Parallel over every resident model submesh (~100k in a city); every write is change-gated.
    let unmarked: bevy::platform::collections::HashSet<Entity> = unmarked.read().collect();
    let unmarked = &unmarked;
    q.par_iter_mut().for_each(
        |(
            entity,
            part,
            xf,
            mut vis,
            fade,
            tag,
            mat,
            aabb,
            group_vis,
            mat_anim,
            exterior,
            far_side,
            card,
        )| {
            if scene_still
                && !unmarked.contains(&entity)
                && !xf.is_changed()
                && !group_vis.as_ref().is_some_and(|g| g.is_changed())
                && !mat_anim.as_ref().is_some_and(|a| a.is_changed())
                && !far_side.as_ref().is_some_and(|f| f.is_changed())
            {
                return;
            }
            let far_side = far_side.is_some();
            let (group_vis, mat_anim) = (group_vis.as_deref(), mat_anim.as_deref());
            let toggled_on =
                m.kind_visible[kind_index(part.kind)] && m.blend_visible[blend_index(part.blend)];
            // `farclip` culls doodads and WMOs only: units and GameObjects come range-limited from
            // the server's visibility stream.
            let distance_culled = matches!(part.kind, ModelKind::Doodad | ModelKind::Wmo);
            let pos = xf.translation();
            // By the bounding sphere's nearest point, so a straddler stays drawn for the per-pixel
            // wall to dissolve. No `Aabb` for a frame after spawn: the origin stands in.
            let in_range = match (distance_culled, cam_pos, cam_fwd) {
                (false, _, _) | (_, None, _) | (_, _, None) => true,
                (true, Some(c), Some(fwd)) => {
                    let (center, radius) = match aabb {
                        Some(a) => (
                            xf.transform_point(Vec3::from(a.center)),
                            Vec3::from(a.half_extents).length()
                                * xf.affine().matrix3.x_axis.length(),
                        ),
                        None => (pos, 0.0),
                    };
                    // The particle draw-set gate reads the same rule, so no emitter outlives its
                    // doodad past the wall.
                    crate::view::within_farclip(view.farclip, c, fwd, center, radius)
                }
            };

            // The size-bucketed distance fade (reference `0x683f80`), measured to the bounding
            // sphere's centre in the horizontal plane (`0x6952a0`, `0x683f80`).
            let fade_alpha = match (fade, cam_pos) {
                (Some(f), Some(c)) => {
                    let center = xf.transform_point(f.local_center);
                    let (dx, dz) = (center.x - c.x, center.z - c.z);
                    doodad_fade_alpha(f.radius, (dx * dx + dz * dz).sqrt())
                }
                _ => 1.0,
            };

            // A WMO group no portal reaches is hidden; a part with no `WmoGroupVis` never is.
            let portal_visible = !m.portal_cull
                || group_vis.is_none_or(|gv| {
                    instances
                        .get(gv.instance)
                        .ok()
                        .is_none_or(|inst| gv.drawn_by(inst))
                });

            // The same flood picks the fog: the group drawer `0x6b5190` and the group-doodad drawer
            // `0x6b62e0` push the interior triple only under the per-group `[0xca7f00]`. `None`
            // off-WMO leaves the bit to the entity classifier.
            let room_fog = group_vis.map(|gv| {
                instances
                    .get(gv.instance)
                    .is_ok_and(|inst| gv.interior_fogged_by(inst))
            });

            // The animated colour-alpha × transparency-weight factor, into the tag below and the
            // cull here: the reference skips a batch whose alpha is ≤ 0 (`0x707b3a`).
            let mat_factor = mat_anim.map_or(1.0, |m| m.current);

            // Inside a WMO, exterior content draws only through a portal window; untagged content
            // (units, GameObjects) and the camera's own building are exempt.
            let own_building = group_vis.is_some_and(|gv| Some(gv.instance) == own_instance);
            let exterior_ok = !exterior || own_building || gate.admits(&xf, aabb);

            // A billboard card draws only while its model does, by the owner's last-frame verdict.
            // Entity-lane cards only: a placement's card is `ExteriorScene` and gated above. An
            // unreadable owner fails open, as that is `face_billboards`' despawn case.
            let owner_hidden = !exterior
                && card
                    .and_then(crate::billboard::BillboardCard::follows)
                    .is_some_and(|o| matches!(card_owners.get(o), Ok(v) if !v.get()));

            // `Inherited`, not `Visible`, so a hidden parent root still hides its children.
            let desired = if toggled_on
                && in_range
                && fade_alpha > 0.0
                && mat_factor > 0.0
                && portal_visible
                && exterior_ok
                && !owner_hidden
            {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
            if *vis != desired {
                *vis = desired;
            }

            // The two tag fields this system owns, in one read-modify-write: the alpha (fade ×
            // material factor) for `DoodadFade` holders and every non-unit `MatAnim`, lit interior
            // props included (their probe payload keeps bits 0..=15 as alpha), and the room's
            // interior-fog bit. The unit lane's alpha is `entities::apply_unit_mat_alpha`'s.
            if let Some(mut tag) = tag {
                let mut bits = tag.0;
                if fade.is_some() || mat_anim.is_some_and(|m| !m.composes_unit_tag()) {
                    let alpha = fade_alpha * mat_factor;
                    // `with_alpha` keeps a visible card dimmed to 0 off the `MeshTag == 0` opaque
                    // sentinel.
                    bits = crate::mesh_tag::with_alpha(bits, alpha);
                }
                if let Some(on) = room_fog {
                    bits = crate::mesh_tag::with_interior_fog(bits, on);
                }
                if tag.0 != bits {
                    tag.0 = bits;
                }
            }
            if let Some(f) = fade {
                if let Some(mut mat) = mat {
                    let base = if fade_alpha < 1.0 {
                        &f.blend
                    } else {
                        &f.cutout
                    };
                    // A feathering doodad beyond the water plane takes its pick's far twin
                    // (`sky_order::FAR_SIDE_BIAS`); an opaque draw settles by depth. A twin not
                    // built yet keeps the base until next frame.
                    let want = if far_side && fade_alpha < 1.0 {
                        far_twins.far_of(base).unwrap_or(base)
                    } else {
                        base
                    };
                    if mat.0 != *want {
                        mat.0 = want.clone();
                    }
                }
            }
        },
    );

    // A building's MLIQ surfaces, on this frame's PVS: the flood's visit callback links a visited
    // group into the liquid list `0xc7cafc` (`0x685f66`), and the liquid pass unlinks each one it
    // draws (`0x684e21`), so a pool draws only in a frame its group is visited.
    for (gv, mut vis, xf, aabb, exterior, tag) in &mut group_only {
        let portal_ok = !m.portal_cull
            || instances
                .get(gv.instance)
                .ok()
                .is_none_or(|inst| gv.drawn_by(inst));
        // The same exterior-window term as the submesh walk.
        let exterior_ok = !exterior || Some(gv.instance) == own_instance || gate.admits(xf, aabb);
        // A building's water rides the building's toggle: the reference's WMO liquid drain
        // `0x684cd0` gates on the "Map objects" bit `[0xc7b2a4] & 0x100`, while the three ADT
        // drains test the "Water" bit `0x1000000`. ADT liquid must not take this toggle.
        let toggled_on = m.kind_visible[kind_index(ModelKind::Wmo)];
        let want = if portal_ok && exterior_ok && toggled_on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
        // The reference's WMO liquid pass pushes interior fog under the same `[0xca7f00]` as the
        // geometry. `liquid.wgsl` ANDs it with the surface's own class.
        if let Some(mut tag) = tag {
            let on = instances
                .get(gv.instance)
                .is_ok_and(|inst| gv.interior_fogged_by(inst));
            let bits = crate::mesh_tag::with_interior_fog(tag.0, on);
            if tag.0 != bits {
                tag.0 = bits;
            }
        }
    }
}

/// `WOW_VIS_TRACE=<label-substring>` prints the verdicts of every model part whose `WorldObject`
/// label contains it (any case), [`VIS_TRACE_HZ`] times a second, with `bound=` in world space:
///
/// - `vis=Hidden`: [`apply_model_visibility`] said no.
/// - `vis=Inherited inh=false`: an ancestor said no (the exterior-scene election, a transport).
/// - `vis=Inherited inh=true view=false`: Bevy's frustum cull dropped it, so its `Aabb` does not
///   describe what it draws.
#[derive(Resource)]
pub struct VisTrace {
    needle: String,
    next_at: f32,
}

/// Trace passes per second.
const VIS_TRACE_HZ: f32 = 4.0;

impl VisTrace {
    /// `None` unless `WOW_VIS_TRACE` names a substring.
    pub(crate) fn from_env() -> Option<Self> {
        std::env::var("WOW_VIS_TRACE")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|needle| Self {
                needle: needle.to_ascii_lowercase(),
                next_at: 0.0,
            })
    }
}

/// What the trace reads per submesh.
type TracedModels<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static crate::interact::WorldObject,
        &'static GlobalTransform,
        &'static Visibility,
        &'static InheritedVisibility,
        // A chain-only node (an anim host root) has no `ViewVisibility` but still traces.
        Option<&'static ViewVisibility>,
        Option<&'static Aabb>,
        // A billboard card inherits nothing, so its `inh=true` means only "no parent".
        Has<crate::billboard::BillboardCard>,
    ),
>;

/// Prints [`VisTrace`]; runs after the authority, so `vis` is this frame's and `view` last frame's.
pub(super) fn trace_model_visibility(
    time: Res<Time>,
    trace: Option<ResMut<VisTrace>>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    q: TracedModels,
) {
    let Some(mut trace) = trace else { return };
    let now = time.elapsed_secs();
    if now < trace.next_at {
        return;
    }
    trace.next_at = now + 1.0 / VIS_TRACE_HZ;
    let cam_t = cam.iter().next();
    let (cam_pos, cam_fwd) = cam_t.map_or((Vec3::ZERO, Vec3::Z), |t| {
        (t.translation(), Vec3::from(t.forward()))
    });
    for (e, object, xf, vis, inherited, view, aabb, card) in &q {
        if !object.label.to_ascii_lowercase().contains(&trace.needle) {
            continue;
        }
        // The world-space bound Bevy's cull tests.
        let (centre, radius) = match aabb {
            Some(a) => (
                xf.transform_point(Vec3::from(a.center)),
                Vec3::from(a.half_extents).length() * xf.affine().matrix3.x_axis.length(),
            ),
            None => (xf.translation(), f32::NAN),
        };
        let depth = (centre - cam_pos).dot(cam_fwd);
        println!(
            "VIS_TRACE {e} {label} vis={vis:?} inh={inh} view={view} card={card} \
             origin=[{ox:.1},{oy:.1},{oz:.1}] \
             bound=[{cx:.1},{cy:.1},{cz:.1}] r={radius:.1} depth={depth:.1} \
             cam=[{px:.1},{py:.1},{pz:.1}]",
            label = object.label,
            inh = inherited.get(),
            // "-": a chain-only node, never drawn.
            view = view.map_or("-".into(), |v| v.get().to_string()),
            ox = xf.translation().x,
            oy = xf.translation().y,
            oz = xf.translation().z,
            cx = centre.x,
            cy = centre.y,
            cz = centre.z,
            px = cam_pos.x,
            py = cam_pos.y,
            pz = cam_pos.z,
        );
    }
}

/// `WOW_LIQUID_TRACE=1` prints, every frame, each building pool's group, that group's PVS bit and
/// the pool's verdict: `vis` is this frame's, `view` (Bevy's frustum cull) last frame's.
#[derive(Resource, Default)]
pub struct LiquidTrace {
    frame: u64,
}

impl LiquidTrace {
    /// `None` unless `WOW_LIQUID_TRACE` is set to anything but empty.
    pub(crate) fn from_env() -> Option<Self> {
        std::env::var_os("WOW_LIQUID_TRACE")
            .filter(|v| !v.is_empty())
            .map(|_| Self::default())
    }
}

/// Prints [`LiquidTrace`]; runs after the authority.
#[allow(clippy::type_complexity)]
pub(super) fn trace_wmo_liquid(
    mut trace: ResMut<LiquidTrace>,
    instances: Query<&WmoPortalInstance>,
    pools: Query<(Entity, &WmoGroupVis, &Visibility, Option<&ViewVisibility>), Without<ModelPart>>,
) {
    trace.frame += 1;
    for (e, gv, vis, view) in &pools {
        let pvs = instances
            .get(gv.instance)
            .ok()
            .map(|inst| gv.drawn_by(inst));
        println!(
            "LIQUID_TRACE frame={} {e} inst={} groups={:?} pvs={} vis={vis:?} view={}",
            trace.frame,
            gv.instance,
            gv.groups,
            pvs.map_or("-".into(), |p| p.to_string()),
            view.map_or("-".into(), |v| v.get().to_string()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::billboard::BillboardCard;
    use benilla_assets::BillboardInfo;
    use benilla_assets::{WmoGroupNav, WmoModel, WmoPortalInfo, WmoPortalRef};
    use benilla_formats::{BillboardKind, ModelBlend};

    #[test]
    fn a_card_follows_its_owners_verdict() {
        let mut app = App::new();
        app.init_resource::<DebugState>()
            .init_resource::<ViewDistance>()
            .init_resource::<crate::wmo_portal::ExteriorWindows>()
            .init_resource::<crate::wmo_portal::CameraInteriorClaim>()
            .init_resource::<super::super::FarSideTwins>()
            .add_systems(Update, apply_model_visibility);
        let owner = app.world_mut().spawn(InheritedVisibility::VISIBLE).id();
        let info = BillboardInfo {
            bone: 0,
            pivot: Vec3::ZERO,
            kind: BillboardKind::Spherical,
            scale_anim: None,
            seq_translations: vec![],
        };
        let card = app
            .world_mut()
            .spawn((
                ModelPart {
                    kind: ModelKind::GameObject,
                    blend: ModelBlend::Blend,
                },
                GlobalTransform::IDENTITY,
                Visibility::Inherited,
                BillboardCard::following(&info, owner),
            ))
            .id();
        app.update();
        assert_eq!(
            *app.world().entity(card).get::<Visibility>().unwrap(),
            Visibility::Inherited,
            "a visible owner leaves its card drawing"
        );
        app.world_mut()
            .entity_mut(owner)
            .insert(InheritedVisibility::HIDDEN);
        app.update();
        assert_eq!(
            *app.world().entity(card).get::<Visibility>().unwrap(),
            Visibility::Hidden,
            "a hidden owner takes its card with it"
        );
        app.world_mut()
            .entity_mut(owner)
            .insert(InheritedVisibility::VISIBLE);
        app.update();
        assert_eq!(
            *app.world().entity(card).get::<Visibility>().unwrap(),
            Visibility::Inherited,
            "a re-shown owner restores its card"
        );
        // An owner going away must not blank the card: that is the despawn case.
        app.world_mut().entity_mut(owner).despawn();
        app.update();
        assert_eq!(
            *app.world().entity(card).get::<Visibility>().unwrap(),
            Visibility::Inherited,
            "an unreadable owner fails OPEN"
        );
    }

    /// The reference's WMO liquid drain `0x684cd0` gates on the "Map objects" bit
    /// `[0xc7b2a4] & 0x100` (`ModelKind::Wmo`), not the ADT drains' "Water" bit `0x1000000`.
    #[test]
    fn a_wmo_pool_rides_the_map_objects_toggle() {
        let mut app = App::new();
        app.init_resource::<DebugState>()
            .init_resource::<ViewDistance>()
            .init_resource::<crate::wmo_portal::ExteriorWindows>()
            .init_resource::<crate::wmo_portal::CameraInteriorClaim>()
            .init_resource::<super::super::FarSideTwins>()
            .add_systems(Update, apply_model_visibility);
        // An instance entity with no `WmoPortalInstance`: `portal_ok` fails open and the toggle is
        // the only term in play.
        let instance = app.world_mut().spawn(()).id();
        let pool = app
            .world_mut()
            .spawn((
                crate::wmo_portal::WmoGroupVis {
                    instance,
                    groups: std::sync::Arc::from([0u16].as_slice()),
                },
                GlobalTransform::IDENTITY,
                Visibility::Inherited,
            ))
            .id();
        app.update();
        assert_eq!(
            *app.world().entity(pool).get::<Visibility>().unwrap(),
            Visibility::Inherited,
            "buildings on: the canal draws"
        );

        app.world_mut()
            .resource_mut::<DebugState>()
            .models
            .kind_visible[kind_index(ModelKind::Wmo)] = false;
        app.update();
        assert_eq!(
            *app.world().entity(pool).get::<Visibility>().unwrap(),
            Visibility::Hidden,
            "buildings off: the water goes with the building, not on without it"
        );

        // The control: the doodad toggle must not reach a pool.
        app.world_mut()
            .resource_mut::<DebugState>()
            .models
            .kind_visible[kind_index(ModelKind::Wmo)] = true;
        app.world_mut()
            .resource_mut::<DebugState>()
            .models
            .kind_visible[kind_index(ModelKind::Doodad)] = false;
        app.update();
        assert_eq!(
            *app.world().entity(pool).get::<Visibility>().unwrap(),
            Visibility::Inherited,
            "the doodad toggle is a different bit and must not touch a building's water"
        );
    }

    /// The flood and the authority over one placement of `model` at the origin, with a pool in
    /// `group` keyed as the spawn site keys it, and a world camera to aim with [`aim`].
    fn pool_app(model: WmoModel, group: u16) -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<DebugState>()
            .init_resource::<ViewDistance>()
            .init_resource::<crate::wmo_portal::ExteriorWindows>()
            .init_resource::<crate::wmo_portal::CameraInteriorClaim>()
            .init_resource::<crate::wmo_portal::CameraWmoFog>()
            .init_resource::<crate::wmo_portal::WmoCullProbe>()
            .init_resource::<crate::terrain_stream::TerrainStreamer>()
            .init_resource::<super::super::FarSideTwins>()
            .init_resource::<Assets<benilla_assets::AdtTile>>()
            .init_resource::<Assets<WmoModel>>()
            .add_systems(
                Update,
                (crate::wmo_portal::compute_wmo_pvs, apply_model_visibility).chain(),
            );
        let groups = model.group_nav.len();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<WmoModel>>()
            .add(model);
        let instance = app
            .world_mut()
            .spawn(WmoPortalInstance {
                handle,
                world_from_local: bevy::math::Affine3A::IDENTITY,
                name_set: 0,
                visible: vec![true; groups],
                interior_fog: vec![false; groups],
                flooded: vec![None; groups],
            })
            .id();
        let pool = app
            .world_mut()
            .spawn((
                WmoGroupVis {
                    instance,
                    groups: std::sync::Arc::from([group].as_slice()),
                },
                Visibility::Inherited,
                GlobalTransform::IDENTITY,
            ))
            .id();
        app.world_mut().spawn((
            WorldCamera,
            Transform::IDENTITY,
            GlobalTransform::IDENTITY,
            Projection::Perspective(PerspectiveProjection {
                fov: 0.9,
                aspect_ratio: 16.0 / 9.0,
                near: 0.1,
                far: 1000.0,
                ..default()
            }),
        ));
        (app, pool)
    }

    /// Puts the world camera at WoW model-space `eye`, looking at `at`, and runs a frame.
    fn aim(app: &mut App, eye: [f32; 3], at: [f32; 3]) {
        use benilla_assets::coords::wow_to_bevy;
        let pose =
            Transform::from_translation(wow_to_bevy(eye)).looking_at(wow_to_bevy(at), Vec3::Y);
        let mut q = app
            .world_mut()
            .query_filtered::<(&mut Transform, &mut GlobalTransform), With<WorldCamera>>();
        let (mut t, mut g) = q.single_mut(app.world_mut()).expect("one world camera");
        *t = pose;
        *g = GlobalTransform::from(pose);
        app.update();
    }

    fn drawn(app: &App, pool: Entity) -> bool {
        *app.world().entity(pool).get::<Visibility>().unwrap() == Visibility::Inherited
    }

    /// Two interior rooms joined by a doorway in the WoW `x = 0` plane (y in [-0.5, 0.5], z in
    /// [0, 2]): `g0`, floored at `z = 0`, holds the camera, and `g1` beyond holds the pool.
    fn two_rooms() -> WmoModel {
        let room = |min: [f32; 3], max: [f32; 3], ref_start: u16| WmoGroupNav {
            flags: 0x2000,
            bbox_min: min,
            bbox_max: max,
            ref_start,
            ref_count: 1,
            area_table_id: 0,
            fog_indices: [0; 4],
            group_liquid: benilla_formats::NO_GROUP_LIQUID,
        };
        WmoModel {
            portal_vertices: vec![
                [0.0, -0.5, 0.0],
                [0.0, 0.5, 0.0],
                [0.0, 0.5, 2.0],
                [0.0, -0.5, 2.0],
            ],
            portal_infos: vec![WmoPortalInfo {
                start_vertex: 0,
                count: 4,
                plane: [1.0, 0.0, 0.0, 0.0],
            }],
            // `side = -1` puts an eye at x < 0 on g0's entering half-space.
            portal_refs: vec![
                WmoPortalRef {
                    portal: 0,
                    group: 1,
                    side: -1,
                },
                WmoPortalRef {
                    portal: 0,
                    group: 0,
                    side: 1,
                },
            ],
            group_nav: vec![
                room([-10.0, -10.0, 0.0], [0.0, 10.0, 3.0], 0),
                room([0.0, -10.0, 0.0], [10.0, 10.0, 3.0], 1),
            ],
            group_collision_tris: vec![
                vec![
                    [[-10.0, -10.0, 0.0], [0.0, -10.0, 0.0], [0.0, 10.0, 0.0]],
                    [[-10.0, -10.0, 0.0], [0.0, 10.0, 0.0], [-10.0, 10.0, 0.0]],
                ],
                Vec::new(),
            ],
            ..WmoModel::default()
        }
    }

    /// A building's pool draws off this frame's flood: the reference links a visited group into
    /// the liquid list `0xc7cafc` (`0x685f66`) and unlinks it as it draws (`0x684e21`), so a
    /// room the flood stops reaching loses its pool that frame, and gets it back with the room.
    #[test]
    fn a_pool_follows_this_frames_pvs() {
        let (mut app, pool) = pool_app(two_rooms(), 1);
        let eye = [-3.0, 0.0, 1.0];
        let room_visible = |app: &mut App| {
            let mut q = app.world_mut().query::<&WmoPortalInstance>();
            q.single(app.world()).expect("one instance").visible[1]
        };

        aim(&mut app, eye, [10.0, 0.0, 1.0]);
        assert!(
            room_visible(&mut app),
            "the flood crosses the doorway ahead"
        );
        assert!(drawn(&app, pool), "the far room's pool draws with its room");

        aim(&mut app, eye, [-10.0, 0.0, 1.0]);
        assert!(!room_visible(&mut app), "the doorway is behind the camera");
        assert!(
            !drawn(&app, pool),
            "a pool whose room the flood no longer visits does not draw, though it drew last frame"
        );

        aim(&mut app, eye, [10.0, 0.0, 1.0]);
        assert!(drawn(&app, pool), "the room is back, and its pool with it");
    }

    /// A building without a portal graph keeps every group visible, so its pool draws though an
    /// area id gives it an instance.
    #[test]
    fn a_portal_less_buildings_pool_draws() {
        let model = WmoModel {
            wmo_id: 63,
            group_nav: vec![WmoGroupNav {
                flags: 0x1000,
                bbox_min: [-5.0; 3],
                bbox_max: [5.0; 3],
                ref_start: 0,
                ref_count: 0,
                area_table_id: 0,
                fog_indices: [0; 4],
                group_liquid: benilla_formats::NO_GROUP_LIQUID,
            }],
            ..WmoModel::default()
        };
        let (mut app, pool) = pool_app(model, 0);
        for at in [[10.0, 0.0, 0.0], [-10.0, 0.0, 0.0]] {
            aim(&mut app, [0.0, 0.0, 20.0], at);
            assert!(drawn(&app, pool), "the fountain's pool draws from any view");
        }
    }

    /// Northshire Abbey's wall fountain, `abbeygate01.wmo`: no portal graph and WMO id 63, so its
    /// placement takes an instance (the spawn site's `has_portals || wmo_id != 0`), and group 0
    /// holds still water, which draws.
    #[test]
    fn the_abbey_wall_fountain_draws_its_pool() {
        let data = benilla_formats::wow_data_or_skip!();
        let chain = benilla_formats::open_chain(&data).expect("open the MPQ chain");
        let path = r"World\wmo\Azeroth\Collidable Doodads\Elwynn\AbbeyGate\abbeygate01.wmo";
        let root_bytes = chain.read(path).expect("abbeygate01.wmo ships");
        let root = benilla_formats::parse_wmo_root(&root_bytes).expect("parse the root");
        let group = chain
            .read(&path.replace(".wmo", "_000.wmo"))
            .expect("its one group ships");
        let portals = root.portals();
        assert!(
            portals.refs.is_empty() && portals.infos.is_empty(),
            "the fountain has no portal graph"
        );
        let wmo_id = benilla_formats::wmo_root_id(&root_bytes);
        assert_eq!(wmo_id, 63, "an area id: the placement spawns an instance");
        assert_eq!(root.group_count(), 1);
        let water = benilla_formats::wmo_group_liquid_mesh(&group).expect("group 0 holds a pool");
        assert_eq!(water.kind, benilla_formats::LiquidKind::Still);
        let header = benilla_formats::wmo_group_header(&group).expect("group header");
        let info = &root.group_infos()[0];
        let model = WmoModel {
            wmo_id,
            group_nav: vec![WmoGroupNav {
                flags: header.flags,
                bbox_min: info.bbox_min,
                bbox_max: info.bbox_max,
                ref_start: header.portal_ref_start,
                ref_count: header.portal_ref_count,
                area_table_id: header.area_table_id,
                fog_indices: header.fog_indices,
                group_liquid: header.group_liquid,
            }],
            group_liquids: vec![Some(water)],
            ..WmoModel::default()
        };
        let (mut app, pool) = pool_app(model, 0);
        for at in [[10.0, 0.0, 0.0], [-10.0, 0.0, 0.0]] {
            aim(&mut app, [0.0, -18.0, 6.0], at);
            assert!(drawn(&app, pool), "the basin holds its water from any view");
        }
    }
}
