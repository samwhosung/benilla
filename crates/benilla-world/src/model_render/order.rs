//! The M2 scene's transparent order (`0x70ae10`, [`benilla_formats::transparent_cmp`]) over the
//! M2 batches in Bevy's one transparent list. Bevy sorts every item by its own centre's view depth
//! plus its material's bias; after that sort, [`order_m2_transparents`] re-keys each M2 batch as
//! the reference does and re-sorts the list, every other item keeping its distance.
//!
//! Under depth fill (`M2UseZFill`, an instance created without flag 1, `0x707a3c`) a batch's first
//! key is its instance's (`0x707dbf`), shared by an attached model (`0x7143ac`); then the copy, the
//! priority plane, the vertex shader, the batch centre (the largest float for a depth-writing
//! batch, `0x707fb3`), the instance, the layer and the opaque order's keys decide. A model frame's
//! scene ([`NoDepthFill`], flags 5 at `0x76ccf5`) keys each batch by its own centre (`0x707dca`).
//!
//! Deviation: the first key is the view-space depth of the instance's origin, not its squared
//! distance (`0x7143e2`): every other transparent in Bevy's one list sorts by view depth, so the
//! M2 batches keep that metric to stay comparable with them.
//!
//! The batch centre is the submesh's authored centre at bind pose, through the batch entity's own
//! frame; the reference carries it through its centre bone's posed matrix (`0x707d80`).

use std::cmp::Ordering;

use bevy::asset::{AssetEvent, UntypedAssetId};
use bevy::core_pipeline::core_3d::Transparent3d;
use bevy::ecs::query::QueryItem;
use bevy::math::Affine3A;
use bevy::pbr::RenderMaterialInstances;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::render_phase::{sort_phase_system, ViewSortedRenderPhases};
use bevy::render::view::ExtractedView;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};

use benilla_assets::materials::WowModelMaterial;
use benilla_formats::{transparent_tie_cmp, BatchOrder, TransparentElement};

/// On a model root whose scene creates it with flags 5, a model frame's (`0x76ccf5`): depth fill
/// is off (`0x707a48`), so each of its batches is keyed by its own centre (`0x707dca`).
#[derive(Component, Clone, Copy, Default)]
pub struct NoDepthFill;

/// On a batch entity, or an entity above it, whose model instance the hierarchy does not name:
/// the instance, whose origin is its key. A static doodad's batches are separate world roots.
#[derive(Component, Clone, Copy)]
pub struct SortInstance(pub Entity);

/// Where an M2 batch entity sits in the transparent order: resolved once from the hierarchy
/// ([`link_m2_batches`]), its points refreshed as it moves ([`refresh_m2_links`]).
#[derive(Component, Clone, PartialEq)]
pub struct M2SortLink {
    /// The entity whose origin is the instance key: the instance, or for an attached model the
    /// model it hangs from, whose key it takes (`0x7143ac`).
    key: Entity,
    /// The instance, for the order's instance key.
    instance: Entity,
    depth_fill: bool,
    /// A depth-prime twin: the reference's depth-fill copy, `+0x8` = 1 (`0x707ff7`).
    fill_copy: bool,
    /// The batch's keys, when its pick geometry carries them (a material shared by batches, a
    /// character composite's, cannot).
    order: Option<BatchOrder>,
    /// The model-local point the entity's frame holds at its origin: a billboard card's pivot.
    mesh_pivot: Vec3,
    key_point: Vec3,
    world: Affine3A,
}

/// [`M2SortLink`] in the render world.
#[derive(Component, Clone)]
pub struct RenderSortLink {
    instance: u64,
    depth_fill: bool,
    fill_copy: bool,
    order: Option<BatchOrder>,
    mesh_pivot: Vec3,
    key_point: Vec3,
    world: Affine3A,
}

impl ExtractComponent for M2SortLink {
    type QueryData = &'static Self;
    type QueryFilter = Changed<Self>;
    type Out = RenderSortLink;

    fn extract_component(link: QueryItem<'_, '_, Self::QueryData>) -> Option<Self::Out> {
        Some(RenderSortLink {
            instance: link.instance.to_bits(),
            depth_fill: link.depth_fill,
            fill_copy: link.fill_copy,
            order: link.order,
            mesh_pivot: link.mesh_pivot,
            key_point: link.key_point,
            world: link.world,
        })
    }
}

/// The hops a walk takes before it stops at what it reached: a guard against a cycle.
const MAX_HOPS: usize = 64;

/// The instance and key entity of `start`: a [`SortInstance`] names both; an attached model
/// ([`crate::model_fade::ParentModel`]) is its own instance on its parent's key; otherwise the top of
/// the hierarchy is both.
fn resolve(
    start: Entity,
    parents: &Query<&ChildOf>,
    attached: &Query<&crate::model_fade::ParentModel>,
    named: &Query<&SortInstance>,
    depth: usize,
) -> (Entity, Entity) {
    let mut cur = start;
    for _ in 0..MAX_HOPS {
        if let Ok(SortInstance(i)) = named.get(cur) {
            return (*i, *i);
        }
        if let Ok(parent) = attached.get(cur) {
            if depth < MAX_HOPS {
                let (key, _) = resolve(parent.0, parents, attached, named, depth + 1);
                return (key, cur);
            }
            return (cur, cur);
        }
        match parents.get(cur) {
            Ok(p) => cur = p.parent(),
            Err(_) => break,
        }
    }
    (cur, cur)
}

/// Whether nothing from `start` up turns depth fill off.
fn depth_fill(
    start: Entity,
    parents: &Query<&ChildOf>,
    off: &Query<(), With<NoDepthFill>>,
) -> bool {
    let mut cur = start;
    for _ in 0..MAX_HOPS {
        if off.contains(cur) {
            return false;
        }
        match parents.get(cur) {
            Ok(p) => cur = p.parent(),
            Err(_) => break,
        }
    }
    true
}

/// Link every new M2 batch entity to its instance.
#[allow(clippy::type_complexity)] // one system's full input set
pub(crate) fn link_m2_batches(
    mut commands: Commands,
    fresh: Query<
        (
            Entity,
            &GlobalTransform,
            Option<&crate::zfill::ZfillTwinOf>,
            Option<&crate::billboard::BillboardCard>,
            Option<&crate::interact::PickMesh>,
        ),
        (With<MeshMaterial3d<WowModelMaterial>>, Without<M2SortLink>),
    >,
    picks: Query<&crate::interact::PickMesh>,
    cards: Query<&crate::billboard::BillboardCard>,
    parents: Query<&ChildOf>,
    attached: Query<&crate::model_fade::ParentModel>,
    named: Query<&SortInstance>,
    off: Query<(), With<NoDepthFill>>,
    globals: Query<&GlobalTransform>,
) {
    for (entity, g, twin, card, pick) in &fresh {
        // A twin draws its part's mesh in its part's frame.
        let (source, card) = match twin {
            Some(t) => (t.part(), cards.get(t.part()).ok()),
            None => (entity, card),
        };
        let pick = match twin {
            Some(t) => picks.get(t.part()).ok(),
            None => pick,
        };
        // A card is a world root that follows its model, or a frame of it.
        let start = if named.contains(source) {
            source
        } else {
            card.and_then(crate::billboard::BillboardCard::follows)
                .unwrap_or(source)
        };
        let (key, instance) = resolve(start, &parents, &attached, &named, 0);
        let key_point = globals
            .get(key)
            .map_or(g.translation(), |k| k.translation());
        commands.entity(entity).insert(M2SortLink {
            key,
            instance,
            depth_fill: depth_fill(start, &parents, &off),
            fill_copy: twin.is_some(),
            order: pick.and_then(|p| p.0.order),
            mesh_pivot: card.map_or(Vec3::ZERO, crate::billboard::BillboardCard::mesh_pivot),
            key_point,
            world: g.affine(),
        });
    }
}

/// Re-read a moved batch's frame and its key's origin; a key moves only with its batches.
pub(crate) fn refresh_m2_links(
    mut moved: Query<(&GlobalTransform, &mut M2SortLink), Changed<GlobalTransform>>,
    globals: Query<&GlobalTransform>,
) {
    for (g, mut link) in &mut moved {
        let key_point = globals
            .get(link.key)
            .map_or(link.key_point, |k| k.translation());
        let world = g.affine();
        if link.key_point != key_point || link.world != world {
            link.key_point = key_point;
            link.world = world;
        }
    }
}

/// Every model material's sort facts in the render world, by asset.
#[derive(Resource, Default)]
struct MaterialSorts(HashMap<UntypedAssetId, MaterialSort>);

#[derive(Clone, Copy)]
struct MaterialSort {
    order: Option<BatchOrder>,
    /// The bias that is a sort rung (the far side of the water, `crate::sky_order`), with a
    /// depth-prime twin's own bias, which the copy key replaces, taken out.
    rung: f32,
}

fn extract_material_sorts(
    mut sorts: ResMut<MaterialSorts>,
    mut events: Extract<MessageReader<AssetEvent<WowModelMaterial>>>,
    assets: Extract<Res<Assets<WowModelMaterial>>>,
) {
    for event in events.read() {
        match *event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::LoadedWithDependencies { id } => match assets.get(id) {
                Some(m) => {
                    let twin = (m.extension.clutter_fade.z as u16) & super::ZFILL_MARKER != 0;
                    let own = if twin { super::ZFILL_SORT_BIAS } else { 0.0 };
                    sorts.0.insert(
                        id.untyped(),
                        MaterialSort {
                            order: m.extension.order,
                            rung: m.base.depth_bias - own,
                        },
                    );
                }
                None => {
                    sorts.0.remove(&id.untyped());
                }
            },
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => {
                sorts.0.remove(&id.untyped());
            }
        }
    }
}

/// One item's place: its distance, then for an M2 batch the order's later keys.
struct Slot {
    distance: f32,
    m2: Option<(BatchOrder, f32, bool, u64)>,
}

impl Slot {
    fn element(&self) -> Option<TransparentElement<'_>> {
        self.m2.as_ref().map(
            |(order, batch_depth, fill_copy, instance)| TransparentElement {
                order,
                key_depth: 0.0,
                fill_copy: *fill_copy,
                batch_depth: *batch_depth,
                instance: *instance,
            },
        )
    }
}

/// The list order: by distance; at one distance the other items first in Bevy's order, then the
/// M2 batches by the transparent order's later keys.
fn slot_cmp(a: &Slot, b: &Slot) -> Ordering {
    a.distance
        .total_cmp(&b.distance)
        .then_with(|| match (a.element(), b.element()) {
            (Some(x), Some(y)) => transparent_tie_cmp(&x, &y),
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        })
}

/// One M2 batch's distance and keys in a view, or `None` for any other item.
fn m2_slot(
    link: &RenderSortLink,
    sort: Option<&MaterialSort>,
    view_depth: impl Fn(Vec3) -> f32,
    eye: Vec3,
) -> Option<(f32, (BatchOrder, f32, bool, u64))> {
    let order = link.order.or(sort.and_then(|s| s.order))?;
    let rung = sort.map_or(0.0, |s| s.rung);
    let c = order.center();
    let local = benilla_assets::coords::wow_to_bevy(c) - link.mesh_pivot;
    let centre = link.world.transform_point3(local);
    let distance = if link.depth_fill {
        view_depth(link.key_point)
    } else {
        view_depth(centre)
    } + rung;
    // `+0x18`: the largest float for a depth-writing batch under depth fill, else the centre's
    // squared distance; an unordered one compares equal in the reference, here 0.
    let batch_depth = if link.depth_fill && order.depth_write {
        f32::MAX
    } else {
        let d = centre.distance_squared(eye);
        if d.is_nan() {
            0.0
        } else {
            d
        }
    };
    Some((
        distance,
        (order, batch_depth, link.fill_copy, link.instance),
    ))
}

/// Re-sort each view's transparent list with the M2 batches in the reference's order.
fn order_m2_transparents(
    mut phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
    views: Query<&ExtractedView>,
    materials: Res<RenderMaterialInstances>,
    sorts: Res<MaterialSorts>,
    links: Query<&RenderSortLink>,
) {
    for view in &views {
        let Some(phase) = phases.get_mut(&view.retained_view_entity) else {
            continue;
        };
        let rangefinder = view.rangefinder3d();
        let eye = view.world_from_view.translation();
        let mut any = false;
        let mut keyed: Vec<(Slot, Transparent3d)> = std::mem::take(&mut phase.items)
            .into_iter()
            .map(|mut item| {
                let slot = links
                    .get(item.entity.0)
                    .ok()
                    .and_then(|link| {
                        let sort = materials
                            .instances
                            .get(&item.entity.1)
                            .and_then(|m| sorts.0.get(&m.asset_id));
                        m2_slot(link, sort, |p| rangefinder.distance(&p), eye)
                    })
                    .map_or(
                        Slot {
                            distance: item.distance,
                            m2: None,
                        },
                        |(distance, m2)| {
                            any = true;
                            item.distance = distance;
                            Slot {
                                distance,
                                m2: Some(m2),
                            }
                        },
                    );
                (slot, item)
            })
            .collect();
        if any {
            keyed.sort_by(|a, b| slot_cmp(&a.0, &b.0));
        }
        phase.items = keyed.into_iter().map(|(_, item)| item).collect();
    }
}

/// Register the links, their extraction and the re-sort. Only extraction reads a link, so the
/// links are taken after `CheckVisibility`, once every pass that places a transform before it
/// (cards, nameplates, raid marks) has run.
pub(crate) fn plugin(app: &mut App) {
    app.add_plugins(ExtractComponentPlugin::<M2SortLink>::default())
        .add_systems(
            PostUpdate,
            (link_m2_batches, refresh_m2_links)
                .chain()
                .after(crate::billboard::BillboardPlace)
                .after(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
        );
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<MaterialSorts>()
            .add_systems(ExtractSchedule, extract_material_sorts)
            .add_systems(
                Render,
                order_m2_transparents
                    .in_set(RenderSystems::PhaseSort)
                    .after(sort_phase_system::<Transparent3d>),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(depth_fill: bool, instance: u64, key: Vec3, order: BatchOrder) -> RenderSortLink {
        RenderSortLink {
            instance,
            depth_fill,
            fill_copy: false,
            order: Some(order),
            mesh_pivot: Vec3::ZERO,
            key_point: key,
            world: Affine3A::IDENTITY,
        }
    }

    /// A batch whose centre is at Bevy `(0, 0, z)`.
    fn at(z: f32, plane: i16, unit: u16) -> BatchOrder {
        let wow = benilla_assets::coords::bevy_to_wow(Vec3::new(0.0, 0.0, z));
        BatchOrder {
            plane,
            unit,
            center: wow.map(f32::to_bits),
            ..Default::default()
        }
    }

    /// The eye at the origin looking down −Z: view depth is `z` itself.
    fn sorted(links: &[RenderSortLink]) -> Vec<usize> {
        let mut slots: Vec<(Slot, usize)> = links
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let (distance, m2) = m2_slot(l, None, |p| p.z, Vec3::ZERO).unwrap();
                (
                    Slot {
                        distance,
                        m2: Some(m2),
                    },
                    i,
                )
            })
            .collect();
        slots.sort_by(|a, b| slot_cmp(&a.0, &b.0));
        slots.into_iter().map(|(_, i)| i).collect()
    }

    /// `HangingSkullLight01` under depth fill: its glow (unit 0, plane 1) sits farther from the
    /// eye than its skull (unit 1, plane 0), so a sort by each batch's own centre draws the glow
    /// first and the skull over it. The instance key ties them, and the plane draws the skull
    /// first and the glow over it.
    #[test]
    fn one_instance_sorts_by_plane_not_by_its_batches_centres() {
        let key = Vec3::new(0.0, 0.0, -10.0);
        let glow = link(true, 1, key, at(-11.0, 1, 0));
        let skull = link(true, 1, key, at(-9.0, 0, 1));
        assert_eq!(sorted(&[glow, skull]), vec![1, 0]);
    }

    /// Two instances keep their keys' order whatever their planes: the far one draws whole first.
    #[test]
    fn the_instance_key_leads_the_plane() {
        let far = link(true, 1, Vec3::new(0.0, 0.0, -30.0), at(-30.0, 5, 0));
        let near = link(true, 2, Vec3::new(0.0, 0.0, -10.0), at(-10.0, 0, 0));
        assert_eq!(sorted(&[near, far]), vec![1, 0]);
    }

    /// A model frame's batches (no depth fill) sort by their own centres before the plane.
    #[test]
    fn without_depth_fill_each_batch_is_its_own_key() {
        let key = Vec3::ZERO;
        let near_low = link(false, 1, key, at(-5.0, 0, 0));
        let far_high = link(false, 1, key, at(-40.0, 4, 1));
        assert_eq!(sorted(&[near_low, far_high]), vec![1, 0]);
    }

    /// A depth-prime twin leads its instance (`+0x8`), whatever its own keys.
    #[test]
    fn a_twin_leads_its_instance() {
        let key = Vec3::new(0.0, 0.0, -10.0);
        let colour = link(true, 1, key, at(-12.0, 0, 0));
        let mut twin = link(true, 1, key, BatchOrder::default());
        twin.fill_copy = true;
        twin.order = Some(at(-8.0, 3, 0));
        assert_eq!(sorted(&[colour, twin]), vec![1, 0]);
    }

    /// A card's centre is carried by its own frame, from the pivot its mesh is centred on.
    #[test]
    fn a_cards_centre_is_measured_from_its_pivot() {
        let mut card = link(false, 1, Vec3::ZERO, at(-3.0, 0, 0));
        card.mesh_pivot = Vec3::new(0.0, 0.0, -3.0);
        card.world = Affine3A::from_translation(Vec3::new(0.0, 0.0, -20.0));
        let (distance, _) = m2_slot(&card, None, |p| p.z, Vec3::ZERO).unwrap();
        assert_eq!(distance, -20.0);
    }

    /// The material's rung (the far side of the water) moves the whole instance.
    #[test]
    fn the_rung_rides_on_the_key() {
        let l = link(true, 1, Vec3::new(0.0, 0.0, -10.0), at(-10.0, 0, 0));
        let sort = MaterialSort {
            order: None,
            rung: -4.0e4,
        };
        let (distance, _) = m2_slot(&l, Some(&sort), |p| p.z, Vec3::ZERO).unwrap();
        assert_eq!(distance, -10.0 - 4.0e4);
    }
}
