//! Picking for the retained pass: a diverted batch has no entity to carry a
//! [`crate::interact::PickMesh`], so the lane answers the ray as a [`PickSource`]. Admission reads
//! the published lists the render node draws from ([`super::render::GxWorld::visible`],
//! `visible_wmos`), so the answer cannot disagree with the pixels.

use std::sync::Arc;

use bevy::prelude::*;

use super::{FaderState, GxCell, StaticGx};
use crate::interact::{ray_member, ray_mesh_bounds, PickSource, RayHit, WorldObject};

impl PickSource for StaticGx {
    fn cast_objects(&self, ray: Ray3d, all_hits: bool, out: &mut Vec<(Arc<WorldObject>, RayHit)>) {
        let (origin, dir) = (ray.origin, *ray.direction);
        // Broad phase, nearest box entry first as in the entity cast: the narrow walk stops once a
        // hit beats every remaining box.
        let mut candidates: Vec<(f32, &GxCell, usize)> = Vec::new();
        for entry in &self.world.visible {
            match entry {
                super::render::GxDoodadVis::Cell(coord) => {
                    let Some(cell) = self.cells.get(coord) else {
                        continue;
                    };
                    gather(cell, origin, dir, &mut candidates, |cell, i| {
                        // A fader draws here only while Steady; exiled, its entities answer.
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

/// Add one region's admitted items to the broad phase; an item without a bound enters at 0 and is
/// always narrow-tested, as in the entity cast.
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

    /// Cast straight down through `tri([0,0,0])`, which bakes into the Bevy XZ plane at y = 0 over
    /// x, z in [-1, 0].
    fn cast(gx: &StaticGx) -> Vec<(String, u32, f32)> {
        let ray = Ray3d::new(Vec3::new(-0.3, 5.0, -0.3), Dir3::NEG_Y);
        let mut out = Vec::new();
        gx.cast_objects(ray, true, &mut out);
        out.into_iter()
            .map(|(o, hit)| (o.label.clone(), o.id, hit.distance))
            .collect()
    }

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
        assert!(cast(&gx).is_empty(), "an unselected cell is not pickable");
        gx.world.visible.push(GxDoodadVis::Cell((0, 0)));
        assert_eq!(
            cast(&gx),
            vec![("World\\test\\fence.m2".to_string(), 4242, 5.0)],
        );
    }

    /// Exiled, the placement draws as entities that carry their own pick, so the lane goes quiet.
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
