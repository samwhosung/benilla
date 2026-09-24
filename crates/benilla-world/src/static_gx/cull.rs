//! The retained pass's per-frame CPU scene walk: admission per cell, WMO group and prop set, the
//! dead-region reap, the fader exile scan and the `WOW_GX_CENSUS` line.

use bevy::prelude::*;

use super::render::GxDoodadVis;
use super::{FaderState, GxFader, StaticGx};

/// Exile hysteresis in yards: entry into the feather is exact, but both exits need the camera this
/// far past the edge, so a camera parked on one cannot flap.
const FADE_HYST: f32 = 1.0;

/// [`FaderState`] without its entity list.
#[derive(Clone, Copy, PartialEq, Debug)]
enum FadeClass {
    Steady,
    Feather,
    Gone,
}

impl GxFader {
    fn class(&self) -> FadeClass {
        match self.state {
            FaderState::Steady => FadeClass::Steady,
            FaderState::Exiled { .. } => FadeClass::Feather,
            FaderState::Gone => FadeClass::Gone,
        }
    }
}

/// Where a fader at horizontal distance `d` moves from `current`, by the entity path's own
/// [`crate::model_fade::doodad_fade_alpha`]; the exits shift the sample by [`FADE_HYST`].
fn fade_step(radius: f32, d: f32, current: FadeClass) -> FadeClass {
    use crate::model_fade::doodad_fade_alpha as alpha;
    match current {
        FadeClass::Steady => {
            let a = alpha(radius, d);
            if a >= 1.0 {
                FadeClass::Steady
            } else if a > 0.0 {
                FadeClass::Feather
            } else {
                FadeClass::Gone
            }
        }
        FadeClass::Feather => {
            if alpha(radius, d + FADE_HYST) >= 1.0 {
                FadeClass::Steady
            } else if alpha(radius, d - FADE_HYST) <= 0.0 {
                FadeClass::Gone
            } else {
                FadeClass::Feather
            }
        }
        FadeClass::Gone => {
            if alpha(radius, d + FADE_HYST) >= 1.0 {
                FadeClass::Steady
            } else if alpha(radius, d) > 0.0 {
                FadeClass::Feather
            } else {
                FadeClass::Gone
            }
        }
    }
}

/// A referrer set's PVS verdict, `WmoGroupVis::drawn_by`'s rule: any named room visible, a missing
/// bit fail-open. An empty set, an unnamed prop, is never portal-culled.
fn set_admitted(visible: &[bool], rooms: &[u16]) -> bool {
    rooms.is_empty()
        || rooms
            .iter()
            .any(|&g| visible.get(usize::from(g)).copied().unwrap_or(true))
}

/// `WOW_GX_FADE_TRACE=1`: one line per exile transition.
fn fade_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_GX_FADE_TRACE").is_some())
}

/// Spawn an exiled placement as `assemble.rs`'s own doodad bundle, seeded with the blend material
/// and live fade alpha that the fade authority owns from the next `Update`.
fn spawn_exile(commands: &mut Commands, f: &GxFader, alpha: f32, admitted: bool) -> Vec<Entity> {
    let mut ents = Vec::with_capacity(f.batches.len());
    for b in &f.batches {
        let mut e = commands.spawn((
            Mesh3d(b.stat_mesh.clone()),
            MeshMaterial3d(if alpha < 1.0 {
                b.blend.clone()
            } else {
                b.cutout.clone()
            }),
            f.transform,
            // A placement is a world root: its Transform is its global.
            GlobalTransform::from(f.transform),
            crate::model_render::ModelPart {
                kind: crate::model_render::ModelKind::Doodad,
                blend: b.blend_mode,
            },
            crate::model_render::EntityPathWhy("exile"),
            crate::interact::PickMesh(b.geometry.clone()),
            bevy::mesh::MeshTag(crate::mesh_tag::alpha_bits(alpha)),
            crate::model_fade::DoodadFade {
                radius: f.radius,
                local_center: f.local_center,
                cutout: b.cutout.clone(),
                blend: b.blend.clone(),
            },
            crate::exterior_cull::ExteriorScene,
            // The placement's own identity: the pick names it alike whichever lane draws it.
            (*f.object).clone(),
        ));
        if admitted {
            e.insert(bevy::camera::visibility::InheritedVisibility::VISIBLE);
        } else {
            e.insert((
                Visibility::Hidden,
                bevy::camera::visibility::InheritedVisibility::HIDDEN,
            ));
        }
        if let Some(aabb) = b.aabb {
            e.insert((aabb, bevy::camera::visibility::NoAutoAabb));
        }
        ents.push(e.id());
    }
    if fade_trace() {
        println!("GX_FADE_SPAWN uid={} ents={ents:?}", f.uid);
    }
    ents
}

/// The CPU scene walk into this frame's visible lists: frustum, farclip and the exterior window
/// gate per cell (conservative: overdraw, never a hole), plus the portal PVS per WMO group, the
/// camera's own building exempt from the gate. A re-admitted exile despawns and clears its kill
/// bit in the same frame.
pub(super) fn cull_cells(
    mut commands: Commands,
    mut gx: ResMut<StaticGx>,
    debug: Res<crate::dev_state::DebugState>,
    view: Res<crate::view::ViewDistance>,
    cam: Query<
        (
            &GlobalTransform,
            &Projection,
            &bevy::camera::primitives::Frustum,
        ),
        With<crate::view::WorldCamera>,
    >,
    windows: Res<crate::wmo_portal::ExteriorWindows>,
    claim: Res<crate::wmo_portal::CameraInteriorClaim>,
    instances: Query<&crate::wmo_portal::WmoPortalInstance>,
) {
    let _t = super::gx_perf_guard(1);
    for e in gx.pending_despawn.drain(..) {
        commands.entity(e).try_despawn();
    }
    let cam_view = cam.iter().next();
    let StaticGx {
        cells,
        wmos,
        props,
        world,
        fade_events,
        ..
    } = &mut *gx;
    // Reap regions whose instance died with its placement; a WMO placement never changes owner
    // tile (the straddler handoff is M2-only).
    wmos.retain(|e, _| instances.contains(*e));
    world.wmos.retain(|e, _| instances.contains(*e));
    props.retain(|e, _| instances.contains(*e));
    world.props.retain(|e, _| instances.contains(*e));
    world.visible.clear();
    world.visible_wmos.clear();
    let Some((cam_t, proj, frustum)) = cam_view else {
        return;
    };
    let cam_pos = cam_t.translation();
    let cam_fwd = Vec3::from(cam_t.forward());
    let gate = crate::exterior_cull::ExteriorGate::build(&windows, Some((cam_t, proj)));
    let m = &debug.models;
    let doodads_on =
        m.kind_visible[crate::model_render::kind_index(crate::model_render::ModelKind::Doodad)];

    // ---- The fader exile scan ----
    let cam_xz = Vec2::new(cam_pos.x, cam_pos.z);
    for (&key, cell) in cells.iter_mut() {
        if cell.faders.is_empty() {
            continue;
        }
        let Some((mn, mx)) = cell.fader_bounds else {
            continue;
        };
        // The camera's distance range to the fader centres against the band union, with the
        // hysteresis margin, so a wholesale verdict holds for every placement.
        let dmin = cam_xz.distance(cam_xz.clamp(mn, mx));
        let corner = Vec2::new(
            if (cam_xz.x - mn.x).abs() > (cam_xz.x - mx.x).abs() {
                mn.x
            } else {
                mx.x
            },
            if (cam_xz.y - mn.y).abs() > (cam_xz.y - mx.y).abs() {
                mn.y
            } else {
                mx.y
            },
        );
        let dmax = cam_xz.distance(corner);
        let (near_min, far_max) = cell.ring;
        let verdict = if dmax < near_min - FADE_HYST {
            Some(true) // every placement steady, with margin
        } else if dmin > far_max + FADE_HYST {
            Some(false) // every placement gone, with margin
        } else {
            None // the ring straddles the cell: walk it
        };
        if verdict.is_some() && verdict == cell.settled && !cell.bits_stale {
            continue;
        }
        // Cell-grain admission for the spawn frame; the entity path's own test runs next Update.
        let admitted = doodads_on
            && world.cells.get(&key).is_none_or(|d| {
                gate.admits(&GlobalTransform::from_translation(d.origin), Some(&d.aabb))
            });
        let mut bits_changed = cell.bits_stale;
        let mut unarmed_left = false;
        for f in cell.faders.values_mut() {
            // Last frame's exile spawns draw from this frame: arm their kill bits now.
            if let FaderState::Exiled { armed, .. } = &mut f.state {
                if !*armed {
                    *armed = true;
                    bits_changed = true;
                }
            }
            let d = Vec2::new(f.center.x, f.center.z).distance(cam_xz);
            let old = f.class();
            let new = fade_step(f.radius, d, old);
            if new == old {
                continue;
            }
            let was_killed = matches!(
                f.state,
                FaderState::Exiled { armed: true, .. } | FaderState::Gone
            );
            if let FaderState::Exiled { ents, .. } =
                std::mem::replace(&mut f.state, FaderState::Steady)
            {
                for e in ents {
                    commands.entity(e).try_despawn();
                }
            }
            f.state = match new {
                FadeClass::Steady => {
                    fade_events[1] += 1;
                    FaderState::Steady
                }
                FadeClass::Gone => {
                    fade_events[2] += 1;
                    FaderState::Gone
                }
                FadeClass::Feather => {
                    fade_events[0] += 1;
                    let alpha = crate::model_fade::doodad_fade_alpha(f.radius, d);
                    // Unarmed until the entities draw, unless it was Gone and is already killed.
                    let armed = old == FadeClass::Gone;
                    unarmed_left |= !armed;
                    FaderState::Exiled {
                        ents: spawn_exile(&mut commands, f, alpha, admitted),
                        armed,
                    }
                }
            };
            let now_killed = matches!(
                f.state,
                FaderState::Exiled { armed: true, .. } | FaderState::Gone
            );
            if was_killed != now_killed {
                bits_changed = true;
            }
            if fade_trace() {
                println!(
                    "GX_FADE cell=({},{}) uid={} {:?}->{:?} d={:.1} r={:.2}",
                    key.0, key.1, f.uid, old, new, d, f.radius
                );
            }
        }
        // An unarmed exile must be revisited next frame whatever the ring says.
        cell.settled = if unarmed_left { None } else { verdict };
        if bits_changed {
            if let Some(draw) = world.cells.get_mut(&key) {
                // Copy-on-write: the region is shared with the render world's extracted clone.
                let draw = std::sync::Arc::make_mut(draw);
                // Rebuilt whole from the states, so a re-bake's new indices cannot drift.
                draw.killed.iter_mut().for_each(|w| *w = 0);
                for f in cell.faders.values() {
                    let killed = match f.state {
                        FaderState::Steady | FaderState::Exiled { armed: false, .. } => false,
                        FaderState::Exiled { armed: true, .. } | FaderState::Gone => true,
                    };
                    if !killed {
                        continue;
                    }
                    for &i in &f.items {
                        let i = usize::from(i);
                        if let Some(w) = draw.killed.get_mut(i / 64) {
                            *w |= 1u64 << (i % 64);
                        }
                    }
                }
                draw.killed_rev = draw.killed_rev.wrapping_add(1);
            }
            cell.bits_stale = false;
        }
    }

    // The building the camera is in is not exterior to itself, for WMO groups and prop sets.
    let own = claim.0.map(|c| c.room.instance);
    // The dev doodad kind toggle hides every cell at once; the blend-layer toggles do not reach
    // this lane. Cells and prop regions, both the M2 scene, sort near-first as one list: the 1.12
    // client's front-to-back band walk (32 bands of 33⅓ yd, `0xc7bd40`) at cell grain.
    if doodads_on {
        let mut admitted: Vec<(u32, GxDoodadVis)> = Vec::new();
        for (&cell, draw) in &world.cells {
            let center = draw.origin + Vec3::from(draw.aabb.center);
            let radius = Vec3::from(draw.aabb.half_extents).length();
            if !crate::view::within_farclip(view.farclip, cam_pos, cam_fwd, center, radius) {
                continue;
            }
            let sphere = bevy::camera::primitives::Sphere {
                center: center.into(),
                radius,
            };
            if !frustum.intersects_sphere(&sphere, false) {
                continue;
            }
            // ADT doodads are exterior scene: from inside a WMO they draw only through a window.
            if !gate.admits(
                &GlobalTransform::from_translation(draw.origin),
                Some(&draw.aabb),
            ) {
                continue;
            }
            // Distance² is non-negative, so its IEEE bits sort exactly like the value.
            admitted.push((
                cam_pos.distance_squared(center).to_bits(),
                GxDoodadVis::Cell(cell),
            ));
        }
        // Prop regions join the list, admitted per referrer set: PVS (`set_admitted`), farclip,
        // frustum, and the exterior gate for named sets only; an unnamed prop is never gated.
        for (&entity, draw) in &world.props {
            // An instance not queryable yet (spawn-command latency) skips this frame.
            let Ok(inst) = instances.get(entity) else {
                continue;
            };
            let mut sel = crate::static_gx::render::GxSel {
                drawn: vec![false; draw.sets.len()],
                fog: vec![false; draw.sets.len()],
            };
            let mut any = false;
            for (set, aabb) in &draw.groups {
                let Some(rooms) = draw.sets.get(usize::from(*set)) else {
                    continue;
                };
                // The fog lane is written for every set, drawn or not: the record sync reads the
                // whole vector. A missing bit reads false (`interior_fogged_by`), not fail-open.
                sel.fog[usize::from(*set)] = rooms.iter().any(|&g| {
                    inst.interior_fog
                        .get(usize::from(g))
                        .copied()
                        .unwrap_or(false)
                });
                if m.portal_cull && !set_admitted(&inst.visible, rooms) {
                    continue;
                }
                let center = draw.origin + Vec3::from(aabb.center);
                let radius = Vec3::from(aabb.half_extents).length();
                if !crate::view::within_farclip(view.farclip, cam_pos, cam_fwd, center, radius) {
                    continue;
                }
                let sphere = bevy::camera::primitives::Sphere {
                    center: center.into(),
                    radius,
                };
                if !frustum.intersects_sphere(&sphere, false) {
                    continue;
                }
                if !rooms.is_empty()
                    && Some(entity) != own
                    && !gate.admits(&GlobalTransform::from_translation(draw.origin), Some(aabb))
                {
                    continue;
                }
                sel.drawn[usize::from(*set)] = true;
                any = true;
            }
            if any {
                let center = draw.origin + Vec3::from(draw.aabb.center);
                admitted.push((
                    cam_pos.distance_squared(center).to_bits(),
                    GxDoodadVis::Prop(entity, sel),
                ));
            }
        }
        admitted.sort_unstable_by_key(|(d, _)| *d);
        world.visible.extend(admitted.into_iter().map(|(_, v)| v));
    }
    // WMO regions: the dev WMO toggle, then per-group admission, near-first.
    if m.kind_visible[crate::model_render::kind_index(crate::model_render::ModelKind::Wmo)] {
        let mut admitted: Vec<(u32, (Entity, crate::static_gx::render::GxSel))> = Vec::new();
        for (&entity, draw) in &world.wmos {
            let Ok(inst) = instances.get(entity) else {
                continue;
            };
            let max_group = draw.groups.iter().map(|(g, _)| *g).max().unwrap_or(0);
            let mut sel = crate::static_gx::render::GxSel {
                drawn: vec![false; usize::from(max_group) + 1],
                fog: vec![false; usize::from(max_group) + 1],
            };
            let mut any = false;
            for (group, aabb) in &draw.groups {
                // The fog lane for every group, drawn or not, as for prop sets.
                sel.fog[usize::from(*group)] = inst
                    .interior_fog
                    .get(usize::from(*group))
                    .copied()
                    .unwrap_or(false);
                // The portal PVS bit, fail-open like `WmoGroupVis::drawn_by`.
                if m.portal_cull
                    && !inst
                        .visible
                        .get(usize::from(*group))
                        .copied()
                        .unwrap_or(true)
                {
                    continue;
                }
                let center = draw.origin + Vec3::from(aabb.center);
                let radius = Vec3::from(aabb.half_extents).length();
                if !crate::view::within_farclip(view.farclip, cam_pos, cam_fwd, center, radius) {
                    continue;
                }
                let sphere = bevy::camera::primitives::Sphere {
                    center: center.into(),
                    radius,
                };
                if !frustum.intersects_sphere(&sphere, false) {
                    continue;
                }
                // Another building is exterior scene; the camera's own is exempt.
                if Some(entity) != own
                    && !gate.admits(&GlobalTransform::from_translation(draw.origin), Some(aabb))
                {
                    continue;
                }
                sel.drawn[usize::from(*group)] = true;
                any = true;
            }
            if any {
                let center = draw.origin + Vec3::from(draw.aabb.center);
                admitted.push((cam_pos.distance_squared(center).to_bits(), (entity, sel)));
            }
        }
        admitted.sort_unstable_by_key(|(d, _)| *d);
        world
            .visible_wmos
            .extend(admitted.into_iter().map(|(_, v)| v));
    }
    // `WOW_GX_CENSUS=1`: a selected item is one would-be entity-path submesh, so the counts read
    // against the entity path's `VIS_CENSUS`; at or above it is coarser grain, far above a leak.
    static CENSUS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *CENSUS.get_or_init(|| std::env::var_os("WOW_GX_CENSUS").is_some())
        && gx.frame.is_multiple_of(64)
    {
        let world = &gx.world;
        let mut cell_items = 0usize;
        let (mut prop_regions, mut prop_items) = (0usize, 0usize);
        for vis in &world.visible {
            match vis {
                GxDoodadVis::Cell(c) => {
                    cell_items += world.cells.get(c).map_or(0, |d| d.draws.len());
                }
                GxDoodadVis::Prop(e, sel) => {
                    prop_regions += 1;
                    if let Some(d) = world.props.get(e) {
                        prop_items += d
                            .draws
                            .iter()
                            .filter(|i| {
                                i.group.is_some_and(|s| {
                                    sel.drawn.get(usize::from(s)).copied().unwrap_or(false)
                                })
                            })
                            .count();
                    }
                }
            }
        }
        let (mut wmo_items, mut wmo_groups) = (0usize, 0usize);
        for (e, sel) in &world.visible_wmos {
            if let Some(d) = world.wmos.get(e) {
                wmo_items += d
                    .draws
                    .iter()
                    .filter(|i| {
                        i.group.is_some_and(|g| {
                            sel.drawn.get(usize::from(g)).copied().unwrap_or(false)
                        })
                    })
                    .count();
            }
            wmo_groups += sel.drawn.iter().filter(|b| **b).count();
        }
        let (mut fs, mut fx, mut fg) = (0usize, 0usize, 0usize);
        for cell in gx.cells.values() {
            for f in cell.faders.values() {
                match f.state {
                    FaderState::Steady => fs += 1,
                    FaderState::Exiled { .. } => fx += 1,
                    FaderState::Gone => fg += 1,
                }
            }
        }
        let ev = gx.fade_events;
        println!(
            "GX_CENSUS cells={}/{} cell_items={cell_items} wmo_regions={}/{} \
             wmo_groups={wmo_groups} wmo_items={wmo_items} \
             prop_regions={prop_regions}/{} prop_items={prop_items} \
             faders={fs}s/{fx}x/{fg}g fade_events={}x/{}s/{}g",
            world.visible.len() - prop_regions,
            world.cells.len(),
            world.visible_wmos.len(),
            world.wmos.len(),
            world.props.len(),
            ev[0],
            ev[1],
            ev[2],
        );
    }
    if super::gx_perf_enabled() && gx.frame.is_multiple_of(64) {
        use std::sync::atomic::Ordering::Relaxed;
        let ms: Vec<f64> = super::GX_PERF
            .iter()
            .map(|a| a.swap(0, Relaxed) as f64 / 64.0 / 1.0e6)
            .collect();
        println!(
            "GX_PERF ms/frame flush={:.3} cull={:.3} publish={:.3} prepare={:.3} node={:.3} \
             arrays_mb={}",
            ms[0],
            ms[1],
            ms[2],
            ms[3],
            ms[4],
            super::GX_VRAM.load(Relaxed) / (1024 * 1024)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_admission_is_any_of_named_rooms_fail_open() {
        let vis = [false, true, false];
        assert!(set_admitted(&vis, &[1]));
        assert!(set_admitted(&vis, &[0, 1]));
        assert!(!set_admitted(&vis, &[0, 2]));
        assert!(set_admitted(&vis, &[7]), "a missing bit fails open");
        assert!(set_admitted(&vis, &[]), "an unnamed prop is never culled");
    }

    #[test]
    fn the_exile_step_enters_exact_and_exits_sticky() {
        // Radius 0 is the 40..50 yd band, `model_fade`'s small-prop bucket.
        let r = 0.0;
        assert_eq!(fade_step(r, 39.0, FadeClass::Steady), FadeClass::Steady);
        assert_eq!(fade_step(r, 40.0, FadeClass::Steady), FadeClass::Steady);
        assert_eq!(fade_step(r, 40.5, FadeClass::Steady), FadeClass::Feather);
        // A teleport past the band end goes straight to Gone.
        assert_eq!(fade_step(r, 60.0, FadeClass::Steady), FadeClass::Gone);
        // Exits need the 1 yd margin: at 40.5 the probe at 41.5 still feathers.
        assert_eq!(fade_step(r, 40.5, FadeClass::Feather), FadeClass::Feather);
        assert_eq!(fade_step(r, 39.0, FadeClass::Feather), FadeClass::Steady);
        assert_eq!(fade_step(r, 50.5, FadeClass::Feather), FadeClass::Feather);
        assert_eq!(fade_step(r, 51.5, FadeClass::Feather), FadeClass::Gone);
        // Gone feathers again as soon as alpha is nonzero, and needs the margin for Steady.
        assert_eq!(fade_step(r, 49.9, FadeClass::Gone), FadeClass::Feather);
        assert_eq!(fade_step(r, 50.0, FadeClass::Gone), FadeClass::Gone);
        assert_eq!(fade_step(r, 38.9, FadeClass::Gone), FadeClass::Steady);
    }
}
