//! The M2 scene's transparent order (`0x70ae10`, [`benilla_formats::transparent_cmp`]) over the
//! M2 batches in Bevy's one transparent list. Bevy sorts every item by its own centre's view depth
//! plus its material's bias; after that sort, [`order_m2_transparents`] takes the M2 batches out,
//! sorts them alone in the reference's order and merges them back among the other items, which
//! keep Bevy's order, by view depth.
//!
//! Under depth fill (`M2UseZFill`, an instance created without flag 1, `0x707a3c`) a batch's first
//! key is its instance's (`0x707dbf`): the squared length of the instance's model-to-view
//! translation (`+0x84`, `0x7143c0`-`0x7143e2`), shared by an attached model (`0x7143ac`). Then the
//! copy, the priority plane, the vertex shader, the batch centre's squared distance (the largest
//! float for a depth-writing batch, `0x707fb3`), the instance, the layer and the opaque order's
//! keys decide. A model frame's scene ([`NoDepthFill`], flags 5 at `0x76ccf5`) keys each batch by
//! its own centre's squared distance (`+0x14` = `+0x18`, `0x707dca`).
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
use benilla_formats::{transparent_cmp, BatchOrder, TransparentElement};

/// On a model root whose scene creates it with flags 5, a model frame's (`0x76ccf5`): depth fill
/// is off (`0x707a48`), so each of its batches is keyed by its own centre (`0x707dca`).
#[derive(Component, Clone, Copy, Default)]
pub struct NoDepthFill;

/// On a batch entity, or an entity above it, whose model instance the hierarchy does not name:
/// the instance, and the point its key is measured from, the instance's origin (`+0xfc`'s
/// translation row, `0x7143c0`). A static doodad's batches are separate world roots.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct SortInstance {
    instance: Entity,
    key: KeyPoint,
}

impl SortInstance {
    /// `entity` is the instance, its origin the key.
    pub fn of(entity: Entity) -> Self {
        Self::at(entity, Some(entity), Vec3::ZERO)
    }

    /// `instance`, keyed at `origin` in `frame`'s space, or in world space without a frame: for
    /// an instance with no entity at its origin, such as a doodad drawn by billboard cards alone.
    pub fn at(instance: Entity, frame: Option<Entity>, origin: Vec3) -> Self {
        Self {
            instance,
            key: KeyPoint { frame, origin },
        }
    }
}

/// The point an instance key is measured from: `origin` in `frame`'s space, world without one.
#[derive(Clone, Copy, Debug, PartialEq)]
struct KeyPoint {
    frame: Option<Entity>,
    origin: Vec3,
}

impl KeyPoint {
    fn world(&self, globals: &Query<&GlobalTransform>) -> Option<Vec3> {
        match self.frame {
            Some(f) => globals.get(f).ok().map(|g| g.transform_point(self.origin)),
            None => Some(self.origin),
        }
    }
}

/// Where an M2 batch entity sits in the transparent order: resolved once from the hierarchy
/// ([`link_m2_batches`]), its points refreshed as it moves ([`refresh_m2_links`]).
#[derive(Component, Clone, PartialEq)]
pub struct M2SortLink {
    /// Where the instance key is measured: the instance's origin, or for an attached model the
    /// origin of the model it hangs from, whose key it takes (`0x7143ac`).
    key: KeyPoint,
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

/// The key point and instance of `start`: a [`SortInstance`] names both; an attached model
/// ([`crate::model_fade::ParentModel`]) is its own instance on its parent's key; otherwise the top of
/// the hierarchy is both, keyed at its origin.
fn resolve(
    start: Entity,
    parents: &Query<&ChildOf>,
    attached: &Query<&crate::model_fade::ParentModel>,
    named: &Query<&SortInstance>,
    depth: usize,
) -> (KeyPoint, Entity) {
    let own = |e: Entity| KeyPoint {
        frame: Some(e),
        origin: Vec3::ZERO,
    };
    let mut cur = start;
    for _ in 0..MAX_HOPS {
        if let Ok(named) = named.get(cur) {
            return (named.key, named.instance);
        }
        if let Ok(parent) = attached.get(cur) {
            if depth < MAX_HOPS {
                let (key, _) = resolve(parent.0, parents, attached, named, depth + 1);
                return (key, cur);
            }
            return (own(cur), cur);
        }
        match parents.get(cur) {
            Ok(p) => cur = p.parent(),
            Err(_) => break,
        }
    }
    (own(cur), cur)
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
        let key_point = key.world(&globals).unwrap_or(g.translation());
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

/// The instance of a placement whose batches are separate world roots, as `0x70af56` needs one:
/// its anim host's root, else its first batch that is no billboard card, each at the placement's
/// origin; else, drawn by cards alone, the first card keyed at the placement, through the anchor
/// the cards follow on a mover or at `origin` in world space. `batches` pairs each batch with
/// whether it is a card, whose own origin is its pivot.
pub(crate) fn placement_instance(
    rig_root: Option<Entity>,
    batches: &[(Entity, bool)],
    card_owner: Option<Entity>,
    origin: Vec3,
) -> Option<SortInstance> {
    if let Some(root) = rig_root {
        return Some(SortInstance::of(root));
    }
    if let Some(&(part, _)) = batches.iter().find(|(_, card)| !card) {
        return Some(SortInstance::of(part));
    }
    let &(first, _) = batches.first()?;
    Some(match card_owner {
        Some(anchor) => SortInstance::at(first, Some(anchor), Vec3::ZERO),
        None => SortInstance::at(first, None, origin),
    })
}

/// Re-read a moved batch's frame and its key's origin; a key moves only with its batches.
pub(crate) fn refresh_m2_links(
    mut moved: Query<(&GlobalTransform, &mut M2SortLink), Changed<GlobalTransform>>,
    globals: Query<&GlobalTransform>,
) {
    for (g, mut link) in &mut moved {
        let key_point = link.key.world(&globals).unwrap_or(link.key_point);
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

/// One M2 batch's keys in a view.
#[derive(Clone, Copy, Debug)]
struct M2Keys {
    /// The material's rung (the far side of the water): a band the whole list sorts by first, as
    /// Bevy's own sort does through the same bias.
    band: f32,
    /// The view depth it is merged among the other items by: its key point's, or without depth
    /// fill its centre's, plus the band.
    merge_depth: f32,
    order: BatchOrder,
    /// `+0x14`, `+0x18`: squared distances, or `+0x18` the largest float.
    key_depth: f32,
    batch_depth: f32,
    fill_copy: bool,
    instance: u64,
}

impl M2Keys {
    fn element(&self) -> TransparentElement<'_> {
        TransparentElement {
            order: &self.order,
            key_depth: self.key_depth,
            fill_copy: self.fill_copy,
            batch_depth: self.batch_depth,
            instance: self.instance,
        }
    }
}

/// The M2 list's order: the band, then the transparent order.
fn m2_cmp(a: &M2Keys, b: &M2Keys) -> Ordering {
    a.band
        .total_cmp(&b.band)
        .then_with(|| transparent_cmp(&a.element(), &b.element()))
}

/// A squared distance as a key; an unordered one compares equal in the reference, here 0, which
/// keeps the sort's order total.
fn squared(a: Vec3, b: Vec3) -> f32 {
    let d = a.distance_squared(b);
    if d.is_nan() {
        0.0
    } else {
        d
    }
}

/// One M2 batch's keys in a view, or `None` for any other item.
fn m2_keys(
    link: &RenderSortLink,
    sort: Option<&MaterialSort>,
    view_depth: impl Fn(Vec3) -> f32,
    eye: Vec3,
) -> Option<M2Keys> {
    let order = link.order.or(sort.and_then(|s| s.order))?;
    let band = sort.map_or(0.0, |s| s.rung);
    let local = benilla_assets::coords::wow_to_bevy(order.center()) - link.mesh_pivot;
    let centre = link.world.transform_point3(local);
    let centre_depth = squared(centre, eye);
    // `+0x14`: the instance's key under depth fill, else the batch's own `+0x18` (`0x707dca`).
    let (key_depth, merge_point) = if link.depth_fill {
        (squared(link.key_point, eye), link.key_point)
    } else {
        (centre_depth, centre)
    };
    // `+0x18`: the largest float for a depth-writing batch under depth fill (`0x707fb3`).
    let batch_depth = if link.depth_fill && order.depth_write {
        f32::MAX
    } else {
        centre_depth
    };
    Some(M2Keys {
        band,
        merge_depth: view_depth(merge_point) + band,
        order,
        key_depth,
        batch_depth,
        fill_copy: link.fill_copy,
        instance: link.instance,
    })
}

/// Merge the M2 list into the other items, each list keeping its own order: at each step the
/// head nearer the back goes first, the other item on a tie.
fn merge<T>(others: Vec<(f32, T)>, m2: Vec<(f32, T)>) -> Vec<T> {
    let mut out = Vec::with_capacity(others.len() + m2.len());
    let mut others = others.into_iter().peekable();
    let mut m2 = m2.into_iter().peekable();
    loop {
        let take_m2 = match (others.peek(), m2.peek()) {
            (Some(o), Some(m)) => m.0 < o.0,
            (None, Some(_)) => true,
            (Some(_), None) => false,
            (None, None) => break,
        };
        let next = if take_m2 { m2.next() } else { others.next() };
        out.extend(next.map(|(_, item)| item));
    }
    out
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
        let mut others = Vec::with_capacity(phase.items.len());
        let mut m2 = Vec::new();
        for mut item in std::mem::take(&mut phase.items) {
            let keys = links.get(item.entity.0).ok().and_then(|link| {
                let sort = materials
                    .instances
                    .get(&item.entity.1)
                    .and_then(|m| sorts.0.get(&m.asset_id));
                m2_keys(link, sort, |p| rangefinder.distance(&p), eye)
            });
            match keys {
                Some(keys) => {
                    item.distance = keys.merge_depth;
                    m2.push((keys, item));
                }
                None => others.push((item.distance, item)),
            }
        }
        if m2.is_empty() {
            phase.items = others.into_iter().map(|(_, item)| item).collect();
            continue;
        }
        m2.sort_by(|a, b| m2_cmp(&a.0, &b.0));
        let m2 = m2
            .into_iter()
            .map(|(keys, item)| (keys.merge_depth, item))
            .collect();
        phase.items = merge(others, m2);
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
    use bevy::ecs::system::RunSystemOnce;

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

    /// A batch whose centre is at Bevy `pos`.
    fn centred(pos: Vec3, plane: i16, unit: u16) -> BatchOrder {
        let wow = benilla_assets::coords::bevy_to_wow(pos);
        BatchOrder {
            plane,
            unit,
            center: wow.map(f32::to_bits),
            ..Default::default()
        }
    }

    /// A batch whose centre is at Bevy `(0, 0, z)`.
    fn at(z: f32, plane: i16, unit: u16) -> BatchOrder {
        centred(Vec3::new(0.0, 0.0, z), plane, unit)
    }

    fn keys(l: &RenderSortLink) -> M2Keys {
        m2_keys(l, None, |p| p.z, Vec3::ZERO).unwrap()
    }

    /// The eye at the origin looking down −Z: view depth is `z` itself.
    fn sorted(links: &[RenderSortLink]) -> Vec<usize> {
        let mut all: Vec<(M2Keys, usize)> = links
            .iter()
            .enumerate()
            .map(|(i, l)| (keys(l), i))
            .collect();
        all.sort_by(|a, b| m2_cmp(&a.0, &b.0));
        all.into_iter().map(|(_, i)| i).collect()
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

    /// The key is the squared distance to the eye (`+0x84`, `0x7143e2`), not the view depth: an
    /// instance off to the side and shallower in view depth is still the farther one.
    #[test]
    fn the_instance_key_is_the_squared_distance_not_the_view_depth() {
        let wide = Vec3::new(30.0, 0.0, -10.0);
        let ahead = Vec3::new(0.0, 0.0, -20.0);
        let side = link(true, 1, wide, centred(wide, 0, 0));
        let deep = link(true, 2, ahead, centred(ahead, 0, 0));
        assert_eq!(sorted(&[deep, side]), vec![1, 0], "1000 before 400");
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
        let k = keys(&card);
        assert_eq!((k.merge_depth, k.key_depth), (-20.0, 400.0));
    }

    /// The material's rung (the far side of the water) moves the whole instance.
    #[test]
    fn the_rung_rides_on_the_key() {
        let l = link(true, 1, Vec3::new(0.0, 0.0, -10.0), at(-10.0, 0, 0));
        let sort = MaterialSort {
            order: None,
            rung: -4.0e4,
        };
        let k = m2_keys(&l, Some(&sort), |p| p.z, Vec3::ZERO).unwrap();
        assert_eq!((k.band, k.merge_depth), (-4.0e4, -10.0 - 4.0e4));
    }

    /// The M2 list goes back among the other items by view depth, each list in its own order.
    #[test]
    fn the_m2_list_merges_among_the_other_items() {
        let others = vec![(-30.0, "far glass"), (-5.0, "near glass")];
        let m2 = vec![(-10.0, "m2 a"), (-20.0, "m2 b"), (-1.0, "m2 c")];
        assert_eq!(
            merge(others, m2),
            vec!["far glass", "m2 a", "m2 b", "near glass", "m2 c"]
        );
    }

    fn batch(world: &mut World, at: Vec3, parent: Option<Entity>) -> Entity {
        let mut e = world.spawn((
            MeshMaterial3d::<WowModelMaterial>(Handle::default()),
            GlobalTransform::from_translation(at),
        ));
        if let Some(p) = parent {
            e.insert(ChildOf(p));
        }
        e.id()
    }

    fn root(world: &mut World, at: Vec3) -> Entity {
        world.spawn(GlobalTransform::from_translation(at)).id()
    }

    /// Each batch is keyed at its instance's origin, whatever entity it hangs from.
    #[test]
    fn the_links_key_each_instance_at_its_origin() {
        let mut world = World::new();
        let v = |x, y, z| Vec3::new(x, y, z);
        // A doodad whose first batch is a card at its pivot: the key is the placement's origin,
        // through its part, not the card's pivot.
        let card = batch(&mut world, v(5.0, 3.0, -20.0), None);
        let part = batch(&mut world, v(0.0, 0.0, -20.0), None);
        let key = placement_instance(None, &[(card, true), (part, false)], None, Vec3::ZERO);
        world.entity_mut(card).insert(key.unwrap());
        world.entity_mut(part).insert(key.unwrap());
        // A doodad drawn by cards alone keys at its placement.
        let lone = batch(&mut world, v(4.0, 1.0, -50.0), None);
        let lone_key = placement_instance(None, &[(lone, true)], None, v(0.0, 0.0, -50.0));
        world.entity_mut(lone).insert(lone_key.unwrap());
        // A body with an item attached at a bone: the item is its own instance on the body's key.
        let body = root(&mut world, v(0.0, 0.0, -10.0));
        let body_batch = batch(&mut world, v(0.0, 0.0, -10.0), Some(body));
        let bone = world
            .spawn((
                GlobalTransform::from_translation(v(1.0, 2.0, -10.0)),
                ChildOf(body),
            ))
            .id();
        let item = world
            .spawn((
                GlobalTransform::from_translation(v(1.0, 2.0, -10.0)),
                ChildOf(bone),
                crate::model_fade::ParentModel(body),
            ))
            .id();
        let item_batch = batch(&mut world, v(1.0, 2.0, -10.0), Some(item));
        // A mount under its rider's unit: its own instance at its own origin.
        let unit = root(&mut world, v(0.0, 0.0, -5.0));
        let mount = world
            .spawn((
                GlobalTransform::from_translation(v(0.0, 2.0, -5.0)),
                ChildOf(unit),
            ))
            .id();
        world.entity_mut(mount).insert(SortInstance::of(mount));
        let mount_batch = batch(&mut world, v(0.0, 2.0, -5.0), Some(mount));
        // A model frame's tile, whose scene has no depth fill, above the model's root.
        let tile = world
            .spawn((
                GlobalTransform::from_translation(v(0.0, 0.0, -2.0)),
                NoDepthFill,
            ))
            .id();
        let framed = root(&mut world, v(0.0, 0.0, -2.0));
        world.entity_mut(framed).insert(ChildOf(tile));
        let framed_batch = batch(&mut world, v(0.0, 0.0, -2.0), Some(framed));

        world.run_system_once(link_m2_batches).unwrap();
        let got = |world: &World, e: Entity| {
            let l = world.get::<M2SortLink>(e).expect("linked");
            (l.instance, l.key_point, l.depth_fill)
        };
        assert_eq!(got(&world, card), (part, v(0.0, 0.0, -20.0), true));
        assert_eq!(got(&world, lone), (lone, v(0.0, 0.0, -50.0), true));
        assert_eq!(got(&world, body_batch), (body, v(0.0, 0.0, -10.0), true));
        assert_eq!(got(&world, item_batch), (item, v(0.0, 0.0, -10.0), true));
        assert_eq!(got(&world, mount_batch), (mount, v(0.0, 2.0, -5.0), true));
        assert!(!got(&world, framed_batch).2, "no depth fill below the tile");
    }
}
