//! `WOW_PICK="<x>,<y>[;<x>,<y>…]"`: the headless inspector. Casts a ray through each screenshot
//! pixel after `WOW_PICK_AT=<secs>` (default 20) and logs every hit along it, nearest first, with
//! the along-ray and perpendicular gap to the previous hit, the incidence angle, the material,
//! bound texture and shading inputs. `WOW_PICK_COUNT=<n>` and `WOW_PICK_EVERY=<secs>` (0 = one
//! per frame) repeat the cast; the cast honours `Visibility`, so a hit that vanishes between casts
//! was culled. Pixels are in `benilla-visual`'s space and divided by the window scale factor, since
//! `viewport_to_world` works in logical units.

use bevy::mesh::MeshTag;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use super::probes::ProbeClock;
use benilla_assets::materials::WowModelMaterial;
use benilla_world::interact::{WorldObject, WorldPick};
use benilla_world::view::WorldCamera;

pub(crate) struct PickProbePlugin;

impl Plugin for PickProbePlugin {
    fn build(&self, app: &mut App) {
        let pixels = std::env::var("WOW_PICK")
            .ok()
            .map(|s| parse_pixels(&s))
            .unwrap_or_default();
        let at = std::env::var("WOW_PICK_AT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20.0);
        let count = std::env::var("WOW_PICK_COUNT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1u32)
            .max(1);
        let every = std::env::var("WOW_PICK_EVERY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        if pixels.is_empty() {
            warn!("pick: no usable pixel in WOW_PICK (want e.g. 1600,900) — inert");
        }
        app.insert_resource(PickProbe {
            pixels,
            count,
            every,
            taken: 0,
            next_at: at,
        })
        .add_systems(Update, fire_pick);
    }
}

#[derive(Resource)]
struct PickProbe {
    /// Screenshot-space pixels to cast through.
    pixels: Vec<Vec2>,
    /// Casts to make (`WOW_PICK_COUNT`), `WOW_PICK_EVERY` seconds apart (0 = one per frame).
    count: u32,
    every: f32,
    taken: u32,
    next_at: f32,
}

fn parse_pixels(spec: &str) -> Vec<Vec2> {
    spec.split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| {
            let (x, y) = s.split_once(',')?;
            Some(Vec2::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
        })
        .collect()
}

/// The filter deciding what a cast may hit; see [`fire_pick`]'s `objects` for why it is both.
type Pickable = Or<(
    With<WorldObject>,
    With<benilla_world::model_render::ModelPart>,
)>;

/// What a hit entity is asked for: identity, batch class, the material carrying the WMO batch
/// order, the per-instance `MeshTag`, and whether it is a camera-facing card. An equipped item's
/// batch carries no [`WorldObject`].
type HitIdentity = (
    Option<&'static WorldObject>,
    Option<&'static benilla_world::model_render::ModelPart>,
    Option<&'static MeshMaterial3d<WowModelMaterial>>,
    Option<&'static MeshTag>,
    Has<benilla_world::billboard::BillboardCard>,
);

/// Every shading input the batch has, as text: the five packed uniform rows and the base colour,
/// per cast, since several move inside one material handle. `tint.w` is the WMO batch-class lane
/// (0 exterior, 1 interior-unlit, 2 the MOCV lerp).
fn shading_of(mat: Option<&WowModelMaterial>) -> String {
    let Some(m) = mat else {
        return "<no WowModelMaterial>".to_string();
    };
    let e = &m.extension;
    let c = m.base.base_color.to_srgba();
    format!(
        "wmo {:.0} int {:.0} fade {:.0}  class {:.1}  tint {:.3},{:.3},{:.3}  \
         sidn {:.3},{:.3},{:.3} win {:.0}  sunsel {:.2} bias {:.0} uv {:.4},{:.4}  \
         clutter {:.1},{:.1},{:.1},{:.1}  base {:.3},{:.3},{:.3},{:.3} {:?}",
        e.model_flags.x,
        // `int`, the light-lane flag: without `wmo` it lights the batch from the SH probe slot its
        // `MeshTag` names; with it, `class` picks the WMO interior lane.
        e.model_flags.z,
        e.model_flags.y,
        e.tint.w,
        e.tint.x,
        e.tint.y,
        e.tint.z,
        e.sidn.x,
        e.sidn.y,
        e.sidn.z,
        e.sidn.w,
        e.sun_scale.x,
        e.sun_scale.y,
        e.sun_scale.z,
        e.sun_scale.w,
        e.clutter_fade.x,
        e.clutter_fade.y,
        e.clutter_fade.z,
        e.clutter_fade.w,
        c.red,
        c.green,
        c.blue,
        c.alpha,
        m.base.alpha_mode,
    )
}

/// Everything needed to turn a hit entity into a line of text.
#[derive(bevy::ecs::system::SystemParam)]
struct HitNames<'w, 's> {
    identity: Query<'w, 's, HitIdentity>,
    materials: Res<'w, Assets<WowModelMaterial>>,
}

fn fire_pick(
    mut probe: ResMut<PickProbe>,
    time: ProbeClock,
    window: Query<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    // What the ray may hit. Equipped items carry no `WorldObject` (it feeds targeting, and a
    // weapon is not a target), so `ModelPart`, on every drawn batch, is accepted too.
    objects: Query<Entity, Pickable>,
    names: HitNames,
    // Everything drawn, entity-owned or not, including the consolidated lanes ([`WorldPick`]).
    pick: WorldPick,
) {
    if probe.taken >= probe.count || probe.pixels.is_empty() || time.elapsed_secs() < probe.next_at
    {
        return;
    }
    let (Ok((camera, cam_tf)), Ok(window)) = (camera.single(), window.single()) else {
        return; // no world camera or window yet
    };
    let cast = probe.taken;
    probe.taken += 1;
    probe.next_at = time.elapsed_secs() + probe.every;
    let scale = window.scale_factor();
    let pickable: HashSet<Entity> = objects.iter().collect();
    for &pixel in &probe.pixels {
        let logical = pixel / scale;
        let Ok(ray) = camera.viewport_to_world(cam_tf, logical) else {
            warn!("pick ({}, {}): outside the viewport", pixel.x, pixel.y);
            continue;
        };
        // Every hit, not the nearest. The cast reads each part's resident geometry: render meshes
        // are `RENDER_WORLD`-only, so a `MeshRayCast` would find no static model.
        let hits = pick.cast(ray, &pickable, true);
        // The camera's global transform, bit-exact: the one `viewport_to_world` built the ray from.
        let eye = cam_tf.translation();
        info!(
            "pick#{cast} ({}, {}) [logical {:.1}, {:.1}]: {} hits  eye [{:08x},{:08x},{:08x}] {:.6?}",
            pixel.x,
            pixel.y,
            logical.x,
            logical.y,
            hits.len(),
            eye.x.to_bits(),
            eye.y.to_bits(),
            eye.z.to_bits(),
            eye,
        );
        // The perpendicular distance to the previous hit's plane is the gap a depth buffer sees;
        // at a grazing angle the along-ray gap is far larger.
        let mut previous: Option<(f32, Vec3, Vec3)> = None;
        for (i, found) in hits.iter().enumerate() {
            let hit = &found.hit;
            let gap = previous.map_or(String::new(), |(d, point, normal): (f32, Vec3, Vec3)| {
                let perp = (hit.point - point).dot(normal).abs();
                format!(
                    "  (+{:.5} yd along the ray, but {:.5} yd PERPENDICULAR to the last)",
                    hit.distance - d,
                    perp,
                )
            });
            previous = Some((hit.distance, hit.point, hit.normal));
            // A consolidated hit (a merge blob member or retained-pass item) has no entity, so its
            // per-entity columns are bare; its draw state lives in the lane's baked record.
            let (part, mat, tag, card) = match found.entity.map(|e| names.identity.get(e)) {
                Some(Ok((_, part, mat, tag, card))) => (part, mat, tag, card),
                Some(Err(_)) => {
                    info!("  {i:2}  {:9.4} yd  <untagged>{gap}", hit.distance);
                    continue;
                }
                None => (None, None, None, false),
            };
            let lane = if found.entity.is_some() {
                ""
            } else {
                " CONSOLIDATED"
            };
            let obj = found.object.as_ref();
            // The WMO batch order rides in the material's `sun_scale.y` (`model_render`); 0 means
            // no bias applied.
            let resolved = mat.and_then(|m| names.materials.get(&m.0));
            let batch = resolved
                .map(|m| m.extension.sun_scale.y as i32)
                .unwrap_or(-1);
            // The texture the hit is bound to this cast.
            let tex = resolved
                .and_then(|m| m.base.base_color_texture.as_ref())
                .map_or("-".to_string(), |h| format!("{:?}", h.id()));
            // The angle to the ray: 90° is face-on; near 0° sub-pixel coverage, not depth, decides.
            let incidence = ray.direction.dot(hit.normal).abs().clamp(0.0, 1.0).asin();
            // An equipped item's batch has no world identity and is named by blend, material and
            // texture. `detail` is the inspector's second line (a prop's lighting lane, an emitter
            // count).
            let (kind, id, label) = obj.map_or_else(
                || ("<worn>".to_string(), String::new(), String::new()),
                |o| {
                    (
                        format!("{:?}", o.kind),
                        format!("#{}", o.id),
                        if o.detail.is_empty() {
                            o.label.clone()
                        } else {
                            format!("{}  [{}]", o.label, o.detail)
                        },
                    )
                },
            );
            info!(
                "  {i:2}  {:9.4} yd  {kind} {id:<10} bias {batch:3}  {:?}{}{lane}  {:5.1}° to the ray  mat {:?}  tex {tex}  {label}{gap}",
                hit.distance,
                part.map(|p| p.blend),
                if card { " CARD" } else { "" },
                incidence.to_degrees(),
                mat.map(|m| m.0.id()),
            );
            // The material's contents, every hit every cast. The tag shares the line because its
            // bits 16..=29 mean different things under the two material laws.
            info!(
                "        {}  tag {}",
                shading_of(resolved),
                tag.map_or_else(
                    || "<none>".to_string(),
                    |t| benilla_world::mesh_tag::describe(t.0)
                ),
            );
        }
    }
}
