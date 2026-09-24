//! The world-model visibility apply system. Each frame it shows/hides every doodad/WMO/creature
//! submesh from the panel's layer/type toggles AND the faithful far-clip wall-cull (nearest-point of
//! the mesh AABB along the camera-forward axis, same plane as the per-pixel wall) plus the small-prop
//! distance fade. The model-`Visibility` authority, with one *ordered* override: the self-avatar
//! first-person hide (`player::apply_self_model_fade`) runs after this set ([`super::ModelVisSet`])
//! and wins on the self body submeshes. Split from the panel UI/state in `super`.

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

/// Show/hide each model submesh from the layer/type toggles **and** the faithful draw-distance cull.
/// The toggles are a dev inspector; the distance test is real fidelity — in 1.12 every doodad/WMO
/// **hard-pops** out past the `farclip` radius from the camera (no fade, no distance-LOD; the cull tests
/// each placement against the `cameraPos ± farclip` AABB). We approximate
/// that AABB with a radial distance to the placement origin, which reads the same in-frame.
///
/// Runs every frame (the camera moves, so this can't be snapshot-gated like a pure toggle), but only
/// **writes** `Visibility` when a submesh's decision actually flips — so the steady-state cost is one
/// squared-distance compare per submesh and no change-detection churn.
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
    // The two edges the per-part skip below cannot read off its own row: a building streaming
    // in (its portal set is new), and any owner's inherited verdict flipping (a card follows it).
    // `Changed`, not `Added`: the PVS writer sets the tick only on a real change of the
    // visible set, and that set moves when a building's asset lands frames after its spawn.
    new_portals: Query<(), Changed<WmoPortalInstance>>,
    owner_flips: Query<(), Changed<InheritedVisibility>>,
    // A part whose far-side mark was REMOVED reads as never marked; the classifier's full walk
    // can strip it under a still camera (review 2026-09-04), and the strip must un-skip it.
    mut unmarked: RemovedComponents<super::FarSideOfWater>,
    // The per-frame WMO portal PVS (computed by `crate::wmo_portal`), read here so the cull composes
    // with the toggles + far-clip in this single Visibility authority rather than fighting it.
    instances: Query<&WmoPortalInstance>,
    // …and, for exactly the same reason, the exterior-scene window gate (decision 0784). It used to
    // write `Visibility` itself in `PostUpdate`, i.e. AFTER this system — so on every object it
    // admitted it silently undid the toggles, the far-clip cull and the portal PVS, including
    // outdoors, where it wrote `Inherited` over all of them every single frame.
    windows: Res<crate::wmo_portal::ExteriorWindows>,
    // The camera's OWN building is not exterior scene to itself: the reference draws the containing
    // map object through the interior portal walk, never through the per-window exterior populate
    // (`[0xc7b748]` is the branch, not a participant). Without this exemption, standing inside
    // Stratholme would window-cull Stratholme's own walls.
    claim: Res<crate::wmo_portal::CameraInteriorClaim>,
    // The water-plane classification (`model_render::classify_water_side`) — read-only here: this
    // system owns the DoodadFade handle pick, so the far-side axis composes INTO that pick (the
    // marker + the twin map) instead of a second writer re-swapping the handle it just wrote —
    // the exact fight decision 0025's one-authority law exists to prevent.
    far_twins: Res<super::FarSideTwins>,
    // Every entity that can OWN a billboard card — the model root a card belongs to, or a joint of
    // its rig, both of which carry the root's propagated verdict. Read-only, and for the same
    // one-authority reason as the two above: a card is a world root, so nothing carries its owner's
    // hide to it, and the mirror that used to live in `billboard::face_billboards` was ordered too
    // late to survive (decision 1409).
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
    // The building's own MLIQ surfaces (canals, dungeon pools, Ragefire's lava). They carry a
    // `WmoGroupVis` like every other piece of their group but no `ModelPart` — they are not model
    // submeshes and take none of the toggle/far-clip/fade rules above. A second QUERY, deliberately
    // not a second SYSTEM: decision 0025 wants one `Visibility` authority, and this keeps it
    // (decision 0689 — a culled room's water must go with the room, same as its furniture).
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
    // The (single) world Camera3d; the egui overlay is a Camera2d. None before it spawns → no distance
    // cull that frame (toggles still apply).
    let cam_view = cam.iter().next();
    let cam_t = cam_view.as_ref().map(|(t, _, _)| t);
    // Every whole-scene input of a part's verdict, still since last frame. With those still, a
    // part whose own row is still too would come out of the walk below with the same
    // `Visibility`, tag and material it already carries — so it skips the walk (decision 1979's
    // floor: ~8.7 k resident parts re-verdicted on every still frame at the Stormwind pin).
    // Both the propagated frame and the seat's own write: this system runs before propagation,
    // so a teleport frame's move is only visible on the local `Transform` (see doodad_anim).
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
    // The exterior gate, built ONCE for the whole walk (it is a handful of 6-plane frusta) and then
    // asked per submesh below — the same value `crate::exterior_cull` asks for the objects it owns.
    let gate = crate::exterior_cull::ExteriorGate::build(
        &windows,
        cam_view.as_ref().map(|(t, p, _)| (&**t, &**p)),
    );
    // The placement the camera is standing in, exempt from its own window gate (see the param).
    let own_instance = claim.0.map(|c| c.room.instance);
    // Parallel: this walks EVERY model submesh in residency — ~100k in a city — and at that N a
    // serial walk alone blew half the 16.7 ms budget (the Stormwind fps hunt). Every write below
    // is change-gated, so the steady state is a pure read fan-out.
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
            // `farclip` is the *world-doodad/WMO* draw distance only. Creatures/GameObjects come from the
            // server's own visibility stream (already range-limited) and have separate unit-draw rules, so
            // they're not culled here.
            let distance_culled = matches!(part.kind, ModelKind::Doodad | ModelKind::Wmo);
            let pos = xf.translation();
            // Cull by the NEAREST point of the submesh's bounding sphere, not its origin: an object
            // straddling the hard far-clip wall stays drawn so the per-pixel wall (terrain/wow_model.wgsl)
            // DISSOLVES it through the boundary, instead of the whole thing popping when its origin crosses
            // the wall (the "snaps at the centre" artifact — worst for big trees/buildings). `Aabb` is the
            // mesh bound (local); transform to world + uniform placement scale. Absent for a frame after
            // spawn ⇒ fall back to origin distance.
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
                    // Planar depth along the camera-forward axis (the SAME coordinate the per-pixel wall
                    // uses) of the bound's nearest point — so the cull and the wall agree and the object
                    // dissolves through the boundary with no pop, even off-centre. The rule itself lives
                    // in `view::within_farclip`; the particle draw-set gate reads the same one, because
                    // an emitter outliving its own doodad past the wall is exactly what B39 reported.
                    crate::view::within_farclip(view.farclip, c, fwd, center, radius)
                }
            };

            // Faithful size-bucketed per-object distance fade (`model_fade::doodad_fade_alpha`, the
            // reference's `FUN_00683f80`). Only doodads/WMOs carry `DoodadFade`. Distance is measured to the
            // model's bounding-sphere CENTRE (origin + the transformed bbox-centre offset), in the
            // **horizontal** ground plane (Bevy XZ) — both verified against `FUN_006952a0`/`FUN_00683f80`.
            let fade_alpha = match (fade, cam_pos) {
                (Some(f), Some(c)) => {
                    let center = xf.transform_point(f.local_center);
                    let (dx, dz) = (center.x - c.x, center.z - c.z);
                    doodad_fade_alpha(f.radius, (dx * dx + dz * dz).sqrt())
                }
                _ => 1.0,
            };

            // WMO portal visibility: a group the camera can't reach through any portal is hidden — the
            // faithful cull (decision 0031), computed per-frame by `crate::wmo_portal` and ANDed in here so
            // it composes with the toggles + far-clip instead of a second Visibility writer. A submesh with
            // no `WmoGroupVis` (every non-WMO entity, and a portal-less WMO) is never portal-culled; the
            // panel's `portal_cull` switch disables it wholesale for an A/B against the old "draw every
            // group" look.
            let portal_visible = !m.portal_cull
                || group_vis.is_none_or(|gv| {
                    instances
                        .get(gv.instance)
                        .ok()
                        .is_none_or(|inst| gv.drawn_by(inst))
                });

            // …and, off the SAME per-frame flood, which fog triple this piece's batch is pushed
            // with: the client's per-group `[0xca7f00]` (the group drawer `0x6b5190` and the
            // group-doodad drawer `0x6b62e0` push the interior triple only under
            // it; everything else inherits `push_fog`'s scene triple). `None` for every non-WMO
            // entity, which leaves `INTERIOR_FOG_BIT` to the entity classifier — a unit's fog is
            // staged by the unit's OWN classification, not by the room's gate.
            let room_fog = group_vis.map(|gv| {
                instances
                    .get(gv.instance)
                    .is_ok_and(|inst| gv.interior_fogged_by(inst))
            });

            // The batch's animated material-alpha factor (decision 0130 phase 2): the sampled
            // colour-alpha × transparency-weight loop, `1.0` for the untracked majority. Multiplied
            // into the tag below, and into the cull here — the real client skips a batch whose
            // combined alpha is ≤ 0 before even reading its blend mode (`0x707b3a`),
            // so a flicker track at 0 hides the batch outright.
            let mat_factor = mat_anim.map_or(1.0, |m| m.current);

            // The exterior-scene window gate (decision 0774's law, decision 0784's placement):
            // standing in a WMO interior, exterior content draws only through a portal window. ANDed
            // in here rather than written by a second system, so it composes with everything above
            // instead of overwriting it. Untagged content (units, GameObjects) is exempt by the
            // carved law; the camera's own building is exempt because it is not exterior to itself.
            let own_building = group_vis.is_some_and(|gv| Some(gv.instance) == own_instance);
            let exterior_ok = !exterior || own_building || gate.admits(&xf, aabb);

            // **A billboard card draws only while the model it is a batch OF draws** (decision
            // 1409). A card is split out to a world root because its transform belongs to the
            // billboard system, so it inherits nothing — and every hide that reaches its model
            // through the scene graph (the exterior-scene election on a net root, a transport's
            // off-map leg) reaches the card not at all. That is how an Orgrimmar bonfire's glow
            // card went on burning over its own culled wood.
            //
            // The owner's `InheritedVisibility` is last propagate's — one frame of lag, the same
            // accepted class as the interior law's. **Only on entity-lane cards**: a world-placement
            // card is tagged `ExteriorScene` and `exterior_ok` above already gates it with the rest
            // of its placement, while its owner is a joint of its own placement rig, which is never
            // hidden apart from the model this system is already hiding whole.
            // Fails OPEN on an unreadable owner: an owner that has gone is `face_billboards`'
            // despawn case, and deciding a card's draw on absent information is how the world
            // flickers.
            let owner_hidden = !exterior
                && card
                    .and_then(crate::billboard::BillboardCard::follows)
                    .is_some_and(|o| matches!(card_owners.get(o), Ok(v) if !v.get()));

            // `Inherited` (not `Visible`) so child submeshes still respect a hidden parent root. A fully
            // faded doodad (`fade_alpha == 0`) is culled just like an out-of-range one.
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

            // Push the fade alpha to the shader (per-instance `MeshTag` alpha field — `wow_model.wgsl`
            // multiplies the cutout alpha by it) and swap to the blend material variant while feathering,
            // back to the cutout once opaque. Write only on change so steady doodads (`fade == 1.0`,
            // already on the cutout material) cost nothing and don't re-batch every frame.
            //
            // The alpha field is written for `DoodadFade` holders (the fade composes `mat_factor` in)
            // AND for every other non-unit-lane `MatAnim`: the parts that own the channel outright
            // (`drives_tag` — spell-effect parts, which have no fade) and the pinned no-fade lane —
            // the lit interior props. The latter used to be skipped ("the tag is a packed colour"),
            // which was true before the 0355 re-lane but stale after it: the probe-slot payload
            // keeps bits 0..=15 as the alpha field precisely so `with_alpha` composes with the slot.
            // Skipping them dropped a batch's authored dimming constant entirely — the Undercity
            // throne room's LD_lightshaft01 (weights const 0.10/0.05, the reference's near-invisible
            // haze) blasted at full brightness (bug B30). Only the unit lane stays out:
            // `entities::apply_unit_mat_alpha` owns that compose, ordered against the interior
            // classifier and the appear-fade.
            // The two tag fields this system owns, written in ONE read-modify-write so they
            // compose instead of racing: the fade alpha, and the room's interior-fog bit.
            if let Some(mut tag) = tag {
                let mut bits = tag.0;
                if fade.is_some() || mat_anim.is_some_and(|m| !m.composes_unit_tag()) {
                    // Glow cards render at AUTHORED brightness (decision 0159 — the dimmer knob died
                    // with the faithful FFXGlow pass; the square-law is what keeps halos in check).
                    let alpha = fade_alpha * mat_factor;
                    // `with_alpha` handles the `MeshTag == 0` opaque-sentinel (a *visible* glow card
                    // dimmed to exactly 0 must not flip to full bright); a distance-faded doodad
                    // (`fade_alpha == 0`) is Hidden anyway, so ≈0 bits there are equally fine.
                    // Conventions: `crate::mesh_tag`.
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
                    // A feathering doodad on the eye's far side of the water plane takes the far
                    // twin of the SAME pick (the water-plane interleave, `sky_order::FAR_SIDE_BIAS`)
                    // — the classification is the marker's, the pick stays this system's. The
                    // cutout never composes: an opaque draw settles against the water by depth. A
                    // map miss = the twin isn't built yet (classify runs on transparent draws
                    // only); keep the base and pick it up next frame.
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

    // The group-only audience — a building's MLIQ surfaces. Serial: a building has a handful of
    // liquid surfaces, not the ~100k submeshes above — and change-gated like every write here.
    //
    // The gate is the **ever-visited latch**, not this frame's PVS: the client's render-record
    // persistence (`0x684fe0` → `0x6b4060` → `0x6b62e0`) draws a visited MLIQ group's
    // liquid every frame with no portal re-check for the rest of the world session — the Great
    // Forge's walkway-level pool stays put when its group drops out of the flood (B65). An index
    // past the latch fails OPEN (portal-less props never latch and must always draw).
    for (gv, mut vis, xf, aabb, exterior, tag) in &mut group_only {
        let portal_ok = !m.portal_cull
            || instances.get(gv.instance).ok().is_none_or(|inst| {
                gv.groups
                    .iter()
                    .any(|&g| inst.liquid_visited.get(g as usize).copied().unwrap_or(true))
            });
        // …and the same exterior-window term as the submesh walk: another building's canal is
        // exterior content, the canal of the building you are standing in is not.
        let exterior_ok = !exterior || Some(gv.instance) == own_instance || gate.admits(xf, aabb);
        // **A building's water rides the building's own toggle** — the reference's WMO liquid drain
        // `0x684cd0` gates on `[0xc7b2a4] & 0x100`, the "Map objects" bit, NOT on the `0x1000000`
        // "Water" bit the three ADT drains test (decision 1657). Our `ModelKind::Wmo` toggle is
        // that bit, and until now it hid a building
        // and left its pool hanging in the air — which is also the shape a mis-placed pool takes, so
        // the one instrument for telling those apart was itself producing the symptom.
        //
        // ADT liquid must NOT take this term and does not: it is `apply_exterior_cull`'s, which
        // composes no toggles. The asymmetry is the reference's own, and it is per-*bit*, not an
        // oversight to tidy up.
        let toggled_on = m.kind_visible[kind_index(ModelKind::Wmo)];
        let want = if portal_ok && exterior_ok && toggled_on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
        // …and the pool's fog lane, off the same per-frame flood as the room's walls (decision
        // 1787). The reference's WMO liquid pass re-submits the interior fog block under the SAME
        // `[0xca7f00]` as the geometry pass, so a room
        // and its water can never disagree; `liquid.wgsl` ANDs this with the surface's own static
        // interior class. Note the term is the FLOOD's bit, not the ever-visited latch above: a
        // pool the latch keeps drawing after its room left the PVS wears the scene fog, which is
        // what the client's per-group toggle does with a group it is not currently walking.
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

/// `WOW_VIS_TRACE=<label-substring>` — **watch one model's placements decide, frame by frame.**
///
/// The `VIS_CENSUS` line counts what is drawn; it cannot answer "that thing vanished when I turned
/// — who dropped it?". Two different systems can hide a model submesh and they fail in opposite
/// directions:
///
/// - **`vis=Hidden`** — [`apply_model_visibility`] above said no: a toggle, the far-clip wall, a
///   fully-faded doodad, an `A ≤ 0` material track, the portal PVS or the exterior window gate.
/// - **`vis=Inherited inh=false`** — *this* part said yes and an **ancestor** said no. For a body
///   part that is the exterior-scene election (decision 1270), which is decided once on the net
///   root and inherits down; it is also how a transport hides its deck. Without this field the
///   line was indistinguishable from the frustum case below — the election arrived after the
///   trace did, and a body vanishing for the right reason would have read as a bound bug.
/// - **`vis=Inherited inh=true view=false`** — everything of ours admitted it and **Bevy's frustum
///   cull** dropped it, which means the entity's `Aabb` does not describe what it draws.
///
/// That second line is the whole of decisions 1259 and 1261 — an animated placement whose bound was
/// its bind pose, and then the same bound silently recomputed at the twin swap — and both were
/// diagnosed by reading code because nothing could be asked. `bound=` is printed in **world space**
/// beside the camera, so "the box is nowhere near the bird" is visible directly rather than inferred.
///
/// Off (the env var unset) this system is one `Option` check per frame and no query iteration.
/// Throttled to [`VIS_TRACE_HZ`]; matching is a case-insensitive substring of the `WorldObject`
/// label (a model path), so `WOW_VIS_TRACE=bird01` follows every bird in residency.
#[derive(Resource)]
pub struct VisTrace {
    needle: String,
    next_at: f32,
}

/// Trace lines per second — enough to watch a verdict flip under a camera swing, sparse enough that
/// a dozen placements don't bury the log.
const VIS_TRACE_HZ: f32 = 4.0;

impl VisTrace {
    /// `None` unless `WOW_VIS_TRACE` names a substring — the whole cost of the instrument when off.
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

/// What the trace reads per submesh: its identity, where it is, both verdicts, and the bound the
/// second of them tested.
type TracedModels<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static crate::interact::WorldObject,
        &'static GlobalTransform,
        &'static Visibility,
        &'static InheritedVisibility,
        // `Option`: a chain-only node (anim host root — `crate::vis_chain`) has no sweep row;
        // it must still appear in the trace rather than silently vanish from the instrument.
        Option<&'static ViewVisibility>,
        Option<&'static Aabb>,
        // Which lane the row is: a world-root billboard CARD inherits nothing, so `inh=true` on it
        // means only "no parent", never "my model is drawn" — the one row where the second field
        // must not be read as the owner's verdict (decision 1409).
        Has<crate::billboard::BillboardCard>,
    ),
>;

/// The trace's printer — see [`VisTrace`]. Runs after the authority so `vis` is this frame's
/// verdict, and reads `ViewVisibility` (last frame's frustum result, which is the freshest a
/// same-frame reader can have) to separate our gates from the cull.
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
        // World-space bound: the entity transform applied to the local box, which is exactly what
        // Bevy's cull tests — so a bound that has parted company with the geometry shows up here as
        // a centre nowhere near what the eye sees.
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
            // "-" = chain-only node, no sweep row (never drawn, so no per-view verdict exists).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::billboard::BillboardCard;
    use benilla_assets::BillboardInfo;
    use benilla_formats::{BillboardKind, ModelBlend};

    /// **A billboard card draws only while the model it is a batch of draws** (decision 1409).
    ///
    /// A card is split out to a world root — its transform belongs to the billboard system — so
    /// nothing carries its owner's hide to it. `billboard::face_billboards` used to mirror that
    /// hide, and its own unit test proved it wrote `Hidden`; the write was thrown away every frame
    /// because `BillboardPlace` is ordered only `before(CheckVisibility)`, so it landed after the
    /// propagation that would have consumed it and this system overwrote it with `Inherited` in
    /// the next `Update`. In Orgrimmar that was a bonfire's glow card burning over its own culled
    /// wood. The term lives here now, where nothing runs after it.
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
        // …and the owner going away must NOT blank the card: that is `face_billboards`' despawn
        // case, and deciding a draw on absent information is how the world flickers.
        app.world_mut().entity_mut(owner).despawn();
        app.update();
        assert_eq!(
            *app.world().entity(card).get::<Visibility>().unwrap(),
            Visibility::Inherited,
            "an unreadable owner fails OPEN"
        );
    }

    /// **A building's pool rides the building's own toggle** (decision 1657) — and ADT liquid must
    /// not, which is the half that makes this a fidelity fact rather than a tidy-up.
    ///
    /// The reference gates its WMO liquid drain `0x684cd0` on `[0xc7b2a4] & 0x100`, the "Map
    /// objects" bit, while the three ADT drains test the separate `0x1000000` "Water" bit.
    /// `ModelKind::Wmo` is our "Map objects" bit, and
    /// without this term switching buildings off left every canal, fountain and dungeon pool
    /// hanging in mid-air — which happens to be exactly what a mis-placed pool looks like, so the
    /// one instrument for telling a water bug from a wall bug was manufacturing the symptom.
    #[test]
    fn a_wmo_pool_rides_the_map_objects_toggle() {
        let mut app = App::new();
        app.init_resource::<DebugState>()
            .init_resource::<ViewDistance>()
            .init_resource::<crate::wmo_portal::ExteriorWindows>()
            .init_resource::<crate::wmo_portal::CameraInteriorClaim>()
            .init_resource::<super::super::FarSideTwins>()
            .add_systems(Update, apply_model_visibility);
        // A portal-less prop's pool: `WmoGroupVis` with no `ModelPart` (the `group_only` audience),
        // and an instance that carries no latch, so `portal_ok` fails open and the toggle is the
        // only term in play.
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

        // The control: the toggle that is NOT the Map-objects bit must not reach a pool. Doodads
        // off leaves the canal alone — a single `kind_visible` bool ANDed carelessly would take it.
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
}
