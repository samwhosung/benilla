//! The retained pass answers the ray — **the lane names what it drew**.
//!
//! Slices 1–2 and B4 moved the static world's geometry off per-placement entities into retained
//! cell/region bakes. The pick declaration ([`crate::interact::PickMesh`] + `WorldObject`) rode
//! those entities, so the divert took it with them: over an absorbed doodad, building or prop the
//! inspector, the GO hover and `WOW_PICK` all answered *nothing*, and 0929's "pick geometry is
//! declared, never inferred" rule made that silence indistinguishable from a correctly transparent
//! surface. `mod.rs`'s B1 note called it out as a known gap and predicted "one side table if
//! missed"; this is that table, held where the geometry already is.
//!
//! The lane implements [`PickSource`] rather than publishing records to a shared index for one
//! reason: **only the lane knows what it actually drew this frame.** Its visibility is a CPU scene
//! walk — frustum + farclip + the exterior window gate at CELL grain, the portal PVS bit per GROUP,
//! the referrer-set OR per prop SET, and the exile kill bit per fader PLACEMENT — none of which is
//! a `ViewVisibility` an outside walker could read. So the admission below is the *published
//! verdict itself* ([`super::render::GxWorld::visible`] / `visible_wmos`, the very lists the render
//! node draws from), not a re-derivation that could disagree with the pixels.
//!
//! Cost is paid only when someone casts: no per-frame bookkeeping exists for this, and the
//! instruments that cast (the armed inspector, `WOW_PICK`) run at most once a frame.

use std::sync::Arc;

use bevy::prelude::*;

use super::{FaderState, GxCell, StaticGx};
use crate::interact::{ray_member, ray_mesh_bounds, PickSource, RayHit, WorldObject};

impl PickSource for StaticGx {
    fn cast_objects(&self, ray: Ray3d, all_hits: bool, out: &mut Vec<(Arc<WorldObject>, RayHit)>) {
        let (origin, dir) = (ray.origin, *ray.direction);
        // Broad phase over every admitted item, nearest box entry first — the entity cast's own
        // shape, so the narrow walk stops as soon as a confirmed hit beats every remaining box.
        let mut candidates: Vec<(f32, &GxCell, usize)> = Vec::new();
        for entry in &self.world.visible {
            match entry {
                super::render::GxDoodadVis::Cell(coord) => {
                    let Some(cell) = self.cells.get(coord) else {
                        continue;
                    };
                    gather(cell, origin, dir, &mut candidates, |cell, i| {
                        // A fader draws retained only while Steady: Exiled means the placement is
                        // feathering as ordinary entities (which carry their own declaration, and
                        // would otherwise be named twice), Gone means nothing is drawn at all.
                        match cell.items[i].fader {
                            Some(uid) => matches!(
                                cell.faders.get(&uid).map(|f| &f.state),
                                Some(FaderState::Steady)
                            ),
                            None => true,
                        }
                    });
                }
                super::render::GxDoodadVis::Prop(instance, sets) => {
                    let Some(region) = self.props.get(instance) else {
                        continue;
                    };
                    gather(region, origin, dir, &mut candidates, |region, i| {
                        region.items[i].prop.as_ref().is_some_and(|p| {
                            sets.drawn.get(usize::from(p.set)).copied().unwrap_or(false)
                        })
                    });
                }
            }
        }
        for (instance, groups) in &self.world.visible_wmos {
            let Some(region) = self.wmos.get(instance) else {
                continue;
            };
            gather(region, origin, dir, &mut candidates, |region, i| {
                region.items[i].wmo.as_ref().is_some_and(|w| {
                    groups
                        .drawn
                        .get(usize::from(w.group))
                        .copied()
                        .unwrap_or(false)
                })
            });
        }
        candidates.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let mut best = f32::INFINITY;
        for (entry, cell, i) in candidates {
            if !all_hits && best < entry {
                break;
            }
            let item = &cell.items[i];
            if let Some(hit) = ray_member(&item.geometry, item.transform, ray) {
                best = best.min(hit.distance);
                out.push((item.object.clone(), hit));
            }
        }
    }
}

/// Add one region's admitted items to the broad-phase list. An item with no build-time bound is
/// admitted at entry 0 and narrow-tested unconditionally — the entity cast's rule for a bound-less
/// part, kept: a missing bound must never un-pick real geometry.
fn gather<'a>(
    cell: &'a GxCell,
    origin: Vec3,
    dir: Vec3,
    out: &mut Vec<(f32, &'a GxCell, usize)>,
    admitted: impl Fn(&GxCell, usize) -> bool,
) {
    for (i, item) in cell.items.iter().enumerate() {
        if !admitted(cell, i) {
            continue;
        }
        let entry = match &item.local_aabb {
            Some(aabb) => {
                let gt = GlobalTransform::from(item.transform);
                match ray_mesh_bounds(origin, dir, aabb, &gt) {
                    Some(t) => t,
                    None => continue,
                }
            }
            None => 0.0,
        };
        out.push((entry, cell, i));
    }
}

#[cfg(test)]
mod tests {
    use super::super::render::GxDoodadVis;
    use super::super::testkit::{batch_of, object, tri};
    use super::super::{FaderState, GxFadeSeed, StaticGx};
    use super::*;
    use crate::interact::PickSource;
    use benilla_formats::ModelBlend;

    /// Cast straight down through `tri([0,0,0])`'s footprint — its baked triangle lies in the
    /// Bevy XZ plane at y = 0 over x,z ∈ [-1, 0] (`wow_to_bevy`).
    fn cast(gx: &StaticGx) -> Vec<(String, u32, f32)> {
        let ray = Ray3d::new(Vec3::new(-0.3, 5.0, -0.3), Dir3::NEG_Y);
        let mut out = Vec::new();
        gx.cast_objects(ray, true, &mut out);
        out.into_iter()
            .map(|(o, hit)| (o.label.clone(), o.id, hit.distance))
            .collect()
    }

    /// **Decision 1534.** An item the cull SELECTED is named, at its own placement identity and
    /// its own geometry — the pick answer the divert used to take away with the entity.
    #[test]
    fn a_selected_cell_item_is_named() {
        let mut gx = StaticGx::default();
        let tree = object(4242);
        assert!(gx.divert(batch_of(
            &tree,
            &tri([0.0; 3]),
            Vec3::ZERO,
            None,
            ModelBlend::Opaque
        )));
        // Nothing is drawn until the cull selects the cell — and an unselected cell must be as
        // transparent to the ray as an unculled entity's `ViewVisibility` makes it.
        assert!(cast(&gx).is_empty(), "an unselected cell is not pickable");
        gx.world.visible.push(GxDoodadVis::Cell((0, 0)));
        assert_eq!(
            cast(&gx),
            vec![("World\\test\\fence.m2".to_string(), 4242, 5.0)],
        );
    }

    /// A fader draws retained only while STEADY. Once it exiles, the placement is drawing as
    /// ordinary entities that carry their own declaration — so the lane must go quiet, or the
    /// same tree is named twice, once by each half of the protocol.
    #[test]
    fn an_exiled_fader_stops_answering() {
        let mut gx = StaticGx::default();
        let (fence, g) = (object(77), tri([0.0; 3]));
        let mut b = batch_of(&fence, &g, Vec3::ZERO, None, ModelBlend::Opaque);
        b.fade = Some(GxFadeSeed {
            radius: 0.4,
            local_center: Vec3::ZERO,
            stat_mesh: Handle::default(),
            aabb: None,
            cutout: Handle::default(),
            blend: Handle::default(),
        });
        assert!(gx.divert(b));
        gx.world.visible.push(GxDoodadVis::Cell((0, 0)));
        assert_eq!(cast(&gx).len(), 1, "steady: the retained item answers");
        gx.cells
            .get_mut(&(0, 0))
            .expect("the cell")
            .faders
            .get_mut(&77)
            .expect("the placement's seed")
            .state = FaderState::Exiled {
            ents: vec![Entity::PLACEHOLDER],
            armed: true,
        };
        assert!(
            cast(&gx).is_empty(),
            "exiled: the entity half owns the answer",
        );
    }
}
