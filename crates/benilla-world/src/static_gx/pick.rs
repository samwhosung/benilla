//! Picking for the retained pass: a diverted batch has no entity to carry a
//! [`crate::interact::PickMesh`], so the lane answers the ray as a [`PickSource`]. Admission reads
//! the published lists the render node draws from ([`super::render::GxWorld::visible`],
//! `visible_wmos`), so the answer cannot disagree with the pixels.

use std::sync::Arc;

use bevy::prelude::*;

use super::{FaderState, GxCell, StaticGx};
use crate::interact::{ray_aabb, ray_member, PickSource, RayHit, WorldObject};

impl PickSource for StaticGx {
    fn cast_objects(&self, ray: Ray3d, all_hits: bool, out: &mut Vec<(Arc<WorldObject>, RayHit)>) {
        self.cast(ray, all_hits, false, out);
    }

    fn nearest_occluder(&self, ray: Ray3d) -> Option<f32> {
        let mut out = Vec::new();
        self.cast(ray, false, true, &mut out);
        out.iter().map(|(_, hit)| hit.distance).reduce(f32::min)
    }
}

impl StaticGx {
    /// The cast over the drawn items, or with `occluders` over the drawn doodad items that occlude
    /// the pick alone ([`super::GxItem::occludes`]). Regions are walked nearest bound first, and
    /// without `all_hits` the walk stops at the first region that starts beyond the best hit.
    fn cast(
        &self,
        ray: Ray3d,
        all_hits: bool,
        occluders: bool,
        out: &mut Vec<(Arc<WorldObject>, RayHit)>,
    ) {
        let (origin, dir) = (ray.origin, *ray.direction);
        let mut regions: Vec<(f32, Region)> = Vec::new();
        let enters = |cell: &GxCell| {
            if cell.pick_unbounded {
                Some(0.0)
            } else {
                cell.pick_bounds
                    .and_then(|(mn, mx)| ray_aabb(origin, dir, mn, mx))
            }
        };
        for entry in &self.world.visible {
            match entry {
                super::render::GxDoodadVis::Cell(coord) => {
                    if let Some(cell) = self.cells.get(coord) {
                        if let Some(t) = enters(cell) {
                            regions.push((t, Region::Cell(cell)));
                        }
                    }
                }
                super::render::GxDoodadVis::Prop(instance, sets) => {
                    if let Some(cell) = self.props.get(instance) {
                        if let Some(t) = enters(cell) {
                            regions.push((t, Region::Prop(cell, sets)));
                        }
                    }
                }
            }
        }
        // WMO group geometry never occludes the pick here: that is the walk collider's.
        if !occluders {
            for (instance, groups) in &self.world.visible_wmos {
                if let Some(cell) = self.wmos.get(instance) {
                    if let Some(t) = enters(cell) {
                        regions.push((t, Region::Wmo(cell, groups)));
                    }
                }
            }
        }
        regions.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let mut best = f32::INFINITY;
        let mut candidates: Vec<(f32, usize)> = Vec::new();
        for (start, region) in regions {
            if !all_hits && best < start {
                break; // every remaining region starts beyond the confirmed nearest hit
            }
            let cell = region.cell();
            candidates.clear();
            // Broad phase, nearest box entry first as in the entity cast.
            for (i, item) in cell.items.iter().enumerate() {
                // The inline box before the admission, which may chase the item's pointers.
                let entry = match item.world_bounds {
                    Some((mn, mx)) => match ray_aabb(origin, dir, mn, mx) {
                        Some(t) => t,
                        None => continue,
                    },
                    None => 0.0,
                };
                if (occluders && !item.occludes) || !region.draws(i) {
                    continue;
                }
                candidates.push((entry, i));
            }
            candidates.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
            for &(entry, i) in &candidates {
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
}

/// One drawn region and what its draw admits per item.
enum Region<'a> {
    /// A cell: a fader draws here only while Steady; exiled, its entities answer.
    Cell(&'a GxCell),
    /// A prop region: an item whose referrer set is drawn.
    Prop(&'a GxCell, &'a super::render::GxSel),
    /// A WMO region: an item whose group is drawn.
    Wmo(&'a GxCell, &'a super::render::GxSel),
}

impl Region<'_> {
    fn cell(&self) -> &GxCell {
        match self {
            Region::Cell(c) | Region::Prop(c, _) | Region::Wmo(c, _) => c,
        }
    }

    /// Whether this frame's draw includes item `i`.
    fn draws(&self, i: usize) -> bool {
        match self {
            Region::Cell(cell) => match cell.items[i].fader {
                Some(uid) => matches!(
                    cell.faders.get(&uid).map(|f| &f.state),
                    Some(FaderState::Steady)
                ),
                None => true,
            },
            Region::Prop(cell, sets) => cell.items[i]
                .prop
                .as_ref()
                .is_some_and(|p| sets.drawn.get(usize::from(p.set)).copied().unwrap_or(false)),
            Region::Wmo(cell, groups) => cell.items[i].wmo.as_ref().is_some_and(|w| {
                groups
                    .drawn
                    .get(usize::from(w.group))
                    .copied()
                    .unwrap_or(false)
            }),
        }
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

    /// The world trace's doodad leg admits a drawn doodad batch of blend 0 alone
    /// ([`crate::interact::occludes_pick`]): an alpha-keyed batch nearer along the ray does not
    /// occlude.
    #[test]
    fn the_occluder_cast_takes_only_opaque_doodad_batches() {
        let mut gx = StaticGx::default();
        let (rock, leaves) = (object(5), object(6));
        let opaque = tri([0.0; 3]);
        let keyed = Arc::new(benilla_formats::RenderSubmesh {
            blend: ModelBlend::AlphaTest,
            ..(*tri([0.0, 0.0, 2.0])).clone()
        });
        assert!(gx.divert(batch_of(
            &rock,
            &opaque,
            Vec3::ZERO,
            None,
            ModelBlend::Opaque
        )));
        assert!(gx.divert(batch_of(
            &leaves,
            &keyed,
            Vec3::ZERO,
            None,
            ModelBlend::AlphaTest
        )));
        gx.world.visible.push(GxDoodadVis::Cell((0, 0)));
        let ray = Ray3d::new(Vec3::new(-0.3, 5.0, -0.3), Dir3::NEG_Y);
        assert_eq!(
            gx.nearest_occluder(ray),
            Some(5.0),
            "the rock, not the leaves 2 yd above it"
        );
        assert_eq!(cast(&gx).len(), 2, "the identified pick still names both");
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
