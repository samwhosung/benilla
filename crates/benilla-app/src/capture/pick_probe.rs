//! `WOW_PICK` — the headless **"what is at this pixel, all the way back"** probe.
//!
//! The flicker instruments localise a defect: `benilla-visual hotspot` hands
//! back a pixel box and says *this* is what would not hold still. Naming what is actually there was
//! then a manual step — read the ADT placements, guess which model the report meant, hope. The
//! interactive inspector ([`benilla_world::interact`]) answers it with a cursor, which an unattended probe
//! does not have.
//!
//! So: `WOW_PICK="<x>,<y>[;<x>,<y>…]"` (+ `WOW_PICK_AT=<secs>`, default 20) casts a ray through each
//! pixel and logs **every** hit along it, nearest first — not just the front one.
//!
//! `WOW_PICK_COUNT=<n>` / `WOW_PICK_EVERY=<secs>` (0 = one per frame) repeat the cast, the same shape
//! as the screenshot burst — because the cast honours `Visibility`, a hit that *vanishes* between
//! adjacent casts is a surface being **culled**, not one losing a depth test. That distinction is
//! invisible to a single cast and to the pixels alike: both look like the surface went away.
//!
//! Reporting the whole ray is the point. A surface that swaps with another between frames has a
//! rival *behind* it, and the gap between hit 0 and hit 1 is the number that decides the diagnosis:
//! an exact tie is a coplanar authoring tie (no depth precision can break it — only a deterministic
//! order or a bias), while a few millimetres is a precision question. The nearest hit alone cannot
//! tell those apart, and which one it is decides the fix.
//!
//! **The gap that decides it is the PERPENDICULAR one, not the distance along the ray** — reading
//! the along-ray gap as the separation is how B38 was mis-diagnosed twice. Where the ray meets a
//! surface obliquely it travels a long way between two planes that are barely apart; at the awning
//! there, hit 0 and hit 1 are 1–2 yd apart *along the ray* and 3–45 cm apart perpendicular. Both
//! are reported, along with each hit's **incidence angle**, because a surface nearly edge-on to the
//! ray is lost or won by sub-pixel coverage rather than by depth, and that is a different defect
//! wearing the same appearance.
//!
//! Each hit also names its **material and bound texture**, so "the same batch drew something else
//! this frame" is visible at all — it is indistinguishable from a depth fight in the pixels, and
//! ruling it out is what leaves the depth story standing.
//!
//! Coordinates are **screenshot pixels** — the same space `benilla-visual` reports boxes in — so a
//! hotspot box can be pasted straight in. They are divided by the window's scale factor here, since
//! `viewport_to_world` works in logical units and a Retina capture is 2× the logical window.

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

/// The filter deciding what a cast may hit — see [`fire_pick`]'s `objects` for why it is both.
type Pickable = Or<(
    With<WorldObject>,
    With<benilla_world::model_render::ModelPart>,
)>;

/// What a hit entity is asked for: its identity, the batch class it draws as, the material
/// carrying the WMO batch order, and its per-instance `MeshTag` (`Option` because a hit need not be a
/// model batch at all — and an equipped item's batch carries no [`WorldObject`]).
type HitIdentity = (
    Option<&'static WorldObject>,
    Option<&'static benilla_world::model_render::ModelPart>,
    Option<&'static MeshMaterial3d<WowModelMaterial>>,
    Option<&'static MeshTag>,
    // Is this hit a camera-FACING batch (decision 0153's world-root card) or the model's own
    // geometry? The two are indistinguishable in the pixels and lead to completely different
    // diagnoses — a card that should be culled vs a submesh drawn with the wrong texture — so the
    // cast says which. (`card` in the line below.)
    Has<benilla_world::billboard::BillboardCard>,
);

/// Every shading input the batch actually has, as text — the five packed uniform rows and the base
/// colour, printed per cast so a value that MOVES inside one material is visible at all.
///
/// This exists because comparing material **handles** across frames — the check that "eliminated" a
/// distance-fade material swap for B38 — is blind to exactly this: the handle is stable while
/// the contents move, and three of these are re-sampled per frame *by design* (`sun_scale.zw`'s UV
/// animation, the tint lane's per-instance clone, the fade twin's alpha). `tint.w` is the WMO
/// interior batch-class lane (0 exterior / 1 interior-unlit / 2 the MOCV lerp) — a term that
/// re-lights every surface of one building together, which is B38's whole footprint.
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
        // `int` is `model_flags.z` — the LIGHT-LANE flag, and it was the one row of the packed
        // uniforms this line did not print. Without `is_wmo` it selects the interior-prop lane,
        // i.e. "this batch is lit by the SH probe its MeshTag names" rather than by the sky; with
        // it, the WMO interior batch-class lanes `class` then splits. A hit that reads
        // `wmo 0 int 1` is asking about a probe slot, and the `slot` reading of the tag beside it
        // is the one that matters — which is the whole diagnosis of B373.
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

/// Everything needed to turn a hit entity into a line of text — bundled so the system keeps one
/// parameter for "describe this hit" rather than two that must always travel together.
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
    // What the ray may hit. `WorldObject` is the world's identity tag (doodads, WMOs, unit BODY
    // parts) — but an EQUIPPED item carries none: the item lane spawns its parts and cards without
    // one, because that component also feeds the mouseover/targeting fallback and a weapon is not a
    // target. So the probe would answer "what is at this pixel" for a character's skin and silently
    // skip its helm, its pauldrons, its weapon and every camera-facing card hanging off them —
    // exactly the geometry a "this bit of my gear looks wrong" report is about (found chasing the
    // Field Marshal pauldron). `ModelPart` rides every drawn batch in the client, so
    // the probe keys on it too and stays a pure instrument: nothing outside this file reads it.
    objects: Query<Entity, Pickable>,
    names: HitNames,
    // Everything drawn, entity-owned or not ([`WorldPick`]). Without the entity-less lanes this
    // probe reports `0 hits` over every absorbed tree, building and prop — which is exactly what
    // it did between 1434's default flip and 1534.
    pick: WorldPick,
) {
    if probe.taken >= probe.count || probe.pixels.is_empty() || time.elapsed_secs() < probe.next_at
    {
        return;
    }
    let (Ok((camera, cam_tf)), Ok(window)) = (camera.single(), window.single()) else {
        return; // no world camera / window yet — try again next frame
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
        // `all_hits` is the whole reason this exists: the nearest hit alone cannot name the rival
        // surface behind it, which is exactly what we came for. The cast reads each part's
        // RESIDENT geometry — the render meshes are `RENDER_WORLD`-only since
        // 0834, so a `MeshRayCast` here reports nothing for any static model.
        let hits = pick.cast(ray, &pickable, true);
        // The camera's GLOBAL transform, bit-exact, on the line that carries the cast index — so
        // "did the camera move on this frame?" is answerable against "was this frame dim?" without
        // aligning two logs by hand. It is deliberately *this* transform: it is the one
        // `viewport_to_world` just used to build the ray, so a converging hit distance can be
        // attributed to the camera or to the geometry rather than left ambiguous. A dump inside
        // `seat_camera` cannot do that job — it prints the local `Transform` before propagation, and
        // it has no frame index, which is how it got read against the wrong 75 frames once already.
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
        // Distance ALONG THE RAY is not the gap that decides a depth fight, and reading it as if it
        // were is how B38 got called "not a depth fight". At a grazing angle the ray travels
        // nearly parallel to both surfaces, so a yard of ray can separate two planes that are a hair
        // apart — and near a line where two planes INTERSECT the perpendicular gap goes to zero
        // while the along-ray gap stays large. So report the perpendicular distance from this hit's
        // point to the previous hit's plane as well: that is the number a depth buffer sees.
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
            // A CONSOLIDATED hit — a merge blob's member or a retained-pass item — has no entity,
            // so the per-entity columns (material, bound texture, mesh tag, blend) are simply not
            // there to read: its draw state lives in the lane's baked record, not in a component.
            // It is still named, posed and ordered like every other hit, which is what a "what is
            // at this pixel" reading is for; `CONSOLIDATED` says why the columns are bare.
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
            // The WMO authored batch order rides in the material's `sun_scale.y` (see
            // `model_render`): 0 means "no bias applied", which for a WMO batch is itself a finding.
            let resolved = mat.and_then(|m| names.materials.get(&m.0));
            let batch = resolved
                .map(|m| m.extension.sun_scale.y as i32)
                .unwrap_or(-1);
            // The material + texture the hit actually draws with, per cast. A surface that swaps
            // appearance while its GEOMETRY stays put (identical hits, identical order) is either
            // being re-lit or bound to something else; only naming the bound texture each frame
            // tells those apart, and "the same batch drew a different texture this frame" is
            // otherwise completely invisible from the pixels.
            let tex = resolved
                .and_then(|m| m.base.base_color_texture.as_ref())
                .map_or("-".to_string(), |h| format!("{:?}", h.id()));
            // How edge-on is this surface to the ray? 90° is face-on; near 0° the triangle is nearly
            // parallel to the view and covers a pixel by a hair, so which of it and whatever lies
            // behind wins a pixel is decided by sub-pixel coverage rather than by depth — a
            // completely different defect from a depth fight, and indistinguishable in the pixels.
            let incidence = ray.direction.dot(hit.normal).abs().clamp(0.0, 1.0).asin();
            // An untagged batch — an equipped item's part or its billboard card — has no world
            // identity of its own; it is named by its blend + material + texture, which is what
            // separates "the pauldron's plate" from "the pauldron's camera-facing trim".
            // `detail` is the inspector's second line — the kind-specific facts the model path can't
            // carry (a WMO prop's lighting lane, an emitter count). Printed here because this probe
            // IS the inspector without a cursor, and a prop that draws wrong usually differs from a
            // right one only in that line (the black Booty Bay arch).
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
            // The contents, not the identity — the whole point (see [`shading_of`]). Printed for
            // every hit on every cast: with `WOW_PICK_EVERY=0` that is a per-frame record of every
            // shading input this batch has, which is the only way a term moving *inside* one
            // material is visible. The per-instance tag rides the same line because bits 16..=29
            // mean different things under the two material laws, so the pair must be read together.
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
