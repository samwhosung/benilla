//! Entity ground shade: a unit, player or GameObject re-samples the terrain MCSH under it as it
//! moves, and its light node's intensity ramps toward the reference's target: 2.5 lit, 0.5 in the
//! chunk's shadow, 1.0 as an interior (`0x69e280`, `0x69e496`, `0x69e4ad`, `0x69e36b`).
//!
//! The reference registers the node at creation (`0x670db0` from `0x613e10`) with intensity
//! `+0xa4` and target `+0xf8` both 1.0 (`0x670e75`, `0x670e7b`); its first sync sets the target.
//! Each loaded tick steps `+0xa4` toward the target by 3.3333333 x the previous frame's time
//! (`0x69e770`, `[0x810808]`) and stops on it, so a unit appears at 1.0 and ramps to 2.5 over about
//! 0.45 s, never snapping. `0x6a7300` scales the diffuse by `+0xa4`, unclamped.
//!
//! [`GroundShade`] sits on the net entity root (the `[obj+0xe0]` node) and its byte goes to every
//! M2 part below it, so a held weapon dims with its wielder: the reference lights attachments from
//! the owner's node.

use benilla_assets::AdtTile;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::interior::{classify_entity_interior, InteriorLit};
use crate::mesh_tag::{exterior_payload, shade_of, with_shade, InteriorProbePayload};
use crate::terrain_stream::{doodad_ground_shade, ShadeResolve, TerrainStreamer};

/// The reference's intensity step per second, `[0x810808]` = 0x40555555 (`0x69e880`).
const INTENSITY_STEP_PER_SEC: f32 = 3.333_333_3;

/// The rate of `t` (0 = intensity 2.5, 1 = 0.5) over the mix's 2.0-wide span.
const SHADE_RAMP_PER_SEC: f32 = INTENSITY_STEP_PER_SEC / 2.0;

/// Squared distance (yd²) a root moves before the MCSH bit is re-sampled, about one MCSH texel.
const RESAMPLE_DIST_SQ: f32 = 0.25;

/// The ambient word's rate, `[0x810804]` = 2.0 colour units/s (`[+0x9c]` → `[+0xf4]`).
const AMBIENT_RAMP_PER_SEC: f32 = 2.0;

/// `t` at intensity 1.0: a new node's intensity and target (`0x670e75`, `0x670e7b`), and an
/// interior's target (`[+0xf8]` = 1.0 at `0x69e36b`, behind the `[+0xc] & 2` interior gate).
const DAYNIGHT_T: f32 = 0.75;

/// `t` at the lit outdoor target, intensity 2.5 (`[0x810a68]`, `0x69e4ad`).
const LIT_T: f32 = 0.0;

/// The settled-ramp epsilon on `t` and each ambient channel, under half a byte.
const RAMP_EPS: f32 = 1.0 / 640.0;

/// An entity root's light node, the chase pair `0x69e770` steps each frame: intensity
/// (`[+0xa4]` → `[+0xf8]`, held as `t`) and the ambient word (`[+0x9c]` → `[+0xf4]`).
#[derive(Component)]
pub struct GroundShade {
    /// The mix, 1 being intensity 0.5: the parts' tag byte and the bake fold's diffuse scale.
    t: f32,
    /// The last MCSH sample's target, [`LIT_T`] or 1; a new node's is [`DAYNIGHT_T`] until sampled.
    target: f32,
    /// Root position at the last sample (the movement gate).
    last_pos: Vec3,
    /// Whether the first sample landed (a node holds 1.0 until its first sync sets the target).
    sampled: bool,
    /// The interior classifier's verdict; indoors [`DAYNIGHT_T`] overrides the MCSH sample.
    pub(crate) indoor: bool,
    /// On an outdoor-class WMO surface (`MOGI & 0x48`), the lit target overrides the MCSH: the
    /// reference's WMO down-ray sets skip-shadow `[node+0xd] |= 0x2` (`0x6a8bc7`, cleared on
    /// terrain at `0x6a8bed`), and the exterior leg then commits 2.5 (`0x69e483` → `0x69e4ad`).
    pub(crate) on_wmo: bool,
    /// The ramped ambient word (0..1 per channel), the bake fold's input; the classifier seeds it.
    pub(crate) ambient: Vec3,
    pub(crate) ambient_target: Vec3,
    /// The last target `WOW_INTERIOR_LOG` printed; starts off-scale so the first one prints.
    logged_target: f32,
    /// The byte the walk last pushed to the parts; while it matches and no other lane rewrote a
    /// tag, the walk is skipped. `None` when fresh or hidden, so a wake re-asserts.
    asserted: Option<u8>,
}

impl Default for GroundShade {
    fn default() -> Self {
        Self {
            t: DAYNIGHT_T,
            target: DAYNIGHT_T,
            last_pos: Vec3::ZERO,
            sampled: false,
            indoor: false,
            on_wmo: false,
            ambient: Vec3::ZERO,
            ambient_target: Vec3::ZERO,
            logged_target: -1.0,
            asserted: None,
        }
    }
}

impl GroundShade {
    /// The committed intensity (`[node+0xa4]`), which the bake fold multiplies its diffuse word by.
    pub(crate) fn intensity(&self) -> f32 {
        2.5 - 2.0 * self.t
    }

    /// The sample's verdict: the lit or shadowed target (`0x69e496`, `0x69e4ad`).
    fn on_sample(&mut self, shadowed: bool, pos: Vec3) {
        self.target = if shadowed { 1.0 } else { LIT_T };
        self.last_pos = pos;
        self.sampled = true;
    }

    /// One tick of the intensity chase toward the effective target, never past it (`0x69e770`).
    fn step_intensity(&mut self, step: f32) {
        self.t = ramp_toward(self.t, self.effective_target(), step);
    }

    /// The chase's target: day/night indoors, lit on a WMO surface, else the kept MCSH sample, for
    /// a unit as for a GameObject (one law, `0x69e4ad`/`0x69e496`).
    fn effective_target(&self) -> f32 {
        if self.indoor {
            DAYNIGHT_T
        } else if self.on_wmo {
            LIT_T
        } else {
            self.target
        }
    }

    /// Whether both chases sit on their effective targets, the classifier's refold gate.
    pub(crate) fn ramps_settled(&self) -> bool {
        (self.t - self.effective_target()).abs() < RAMP_EPS
            && (self.ambient - self.ambient_target).abs().max_element() < RAMP_EPS
    }

    /// Seed the ambient chase on entering the footprint bake, from the scene ambient toward the
    /// floor's cap-96 word, as the reference's node carries `[+0x9c]` across the leg flip.
    pub(crate) fn seed_ambient(&mut self, from: Vec3, target: Vec3) {
        self.ambient = from;
        self.ambient_target = target;
    }
}

/// A WMO doodad prop on a streamed GameObject (a transport's cargo), lit by its own `CMapDoodadDef`
/// (`0x6a8050`), never the host's light node, so the walk passes it by. In the reference a
/// transport's WMO is in the global map-object list (`0xca7d98`), and a def's `[def+0xa4]` has four
/// writers, none the node: `0x6a7d73` (0.0), `0x695bb3` and `0x6b01bb` (1.0), and `0x698cb4`
/// (0.5), an MCSH sample taken once at residency (`0x698c50`).
#[derive(Component)]
pub struct DoodadDefLit;

/// Roots owed a re-assert at an unchanged byte: another lane (the classifier's exterior reclaim, a
/// newly added part) rewrote a part's `MeshTag` since [`detect_shade_reclaims`] ran.
#[derive(Resource, Default)]
pub(crate) struct ShadeDirtyRoots(bevy::ecs::entity::EntityHashSet);

/// Flag the root above every part whose `MeshTag` changed; its own system, since the walk holds
/// `&mut MeshTag`. The walk's own writes echo back for one frame, which then writes nothing.
pub(crate) fn detect_shade_reclaims(
    changed_parts: Query<Entity, (Changed<MeshTag>, Without<crate::billboard::BillboardCard>)>,
    shade_roots: Query<(), With<GroundShade>>,
    child_of: Query<&ChildOf>,
    mut dirty: ResMut<ShadeDirtyRoots>,
) {
    dirty.0.clear();
    for part in &changed_parts {
        let mut node = part;
        loop {
            if shade_roots.contains(node) {
                dirty.0.insert(node);
                break;
            }
            let Ok(up) = child_of.get(node) else { break };
            node = up.parent();
        }
    }
}

/// `WOW_SHADE_CENSUS=<secs>` (unparseable: 5): a periodic count of the [`DoodadDefLit`] parts and
/// of the tagged parts under a shade root, the marked ones among them.
fn shade_census_every() -> Option<f32> {
    static EVERY: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *EVERY.get_or_init(|| {
        std::env::var("WOW_SHADE_CENSUS")
            .ok()
            .map(|v| v.parse().unwrap_or(5.0))
    })
}

/// The `WOW_SHADE_CENSUS` printer.
fn census_shade_marks(
    marked: Query<(), With<DoodadDefLit>>,
    roots: Query<Entity, With<GroundShade>>,
    children: Query<&Children>,
    tagged: Query<(), With<MeshTag>>,
    time: Res<Time>,
    mut next: Local<f32>,
) {
    let Some(every) = shade_census_every() else {
        return;
    };
    let now = time.elapsed_secs();
    if now < *next {
        return;
    }
    *next = now + every;
    let mut under_roots = 0usize;
    let mut marked_under_roots = 0usize;
    for r in &roots {
        for e in children.iter_descendants(r) {
            if tagged.get(e).is_ok() {
                under_roots += 1;
                if marked.get(e).is_ok() {
                    marked_under_roots += 1;
                }
            }
        }
    }
    info!(
        "shade-census: DoodadDefLit total {} | tagged parts under a shade root {under_roots}, of which marked {marked_under_roots}",
        marked.iter().count(),
    );
}

pub(crate) struct EntityShadePlugin;

impl Plugin for EntityShadePlugin {
    fn build(&self, app: &mut App) {
        // After the classifier, whose exterior reclaim writes shade byte 0 the same frame this
        // re-asserts; the detector sits between them to see the reclaim that frame.
        app.init_resource::<ShadeDirtyRoots>().add_systems(
            Update,
            (
                detect_shade_reclaims,
                update_ground_shade,
                census_shade_marks,
            )
                .chain()
                .after(classify_entity_interior),
        );
    }
}

/// Sample and ramp each shaded root, then push its byte to its parts' tags.
#[allow(clippy::type_complexity)]
pub(crate) fn update_ground_shade(
    time: Res<Time>,
    streamer: Option<Res<TerrainStreamer>>,
    adt_tiles: Res<Assets<AdtTile>>,
    mut roots: Query<(
        Entity,
        &GlobalTransform,
        &mut GroundShade,
        Option<&crate::world_unit::ViewerUnit>,
        // The election's verdict; `Option`, since a fixed doodad's shade root has no `Visibility`.
        Option<&Visibility>,
    )>,
    children: Query<&Children>,
    // A fading part is walked (shade and fade own disjoint fields); a probe-slot one is not.
    mut parts: Query<
        (
            &mut MeshTag,
            Option<&InteriorLit>,
            Has<InteriorProbePayload>,
            Has<DoodadDefLit>,
        ),
        Without<crate::billboard::BillboardCard>,
    >,
    // Cards are world roots the walk cannot reach; each resolves up from its owner instead.
    mut cards: Query<(
        &crate::billboard::BillboardCard,
        &mut MeshTag,
        Option<&InteriorLit>,
        Has<InteriorProbePayload>,
        Has<DoodadDefLit>,
    )>,
    // Each shaded root's byte, kept across frames (entity ids are generational).
    mut root_shade: Local<bevy::ecs::entity::EntityHashMap<u8>>,
    // The card pass's up-walk: a card can follow a deep joint, which is not a shade root.
    child_of: Query<&ChildOf>,
    dirty_roots: Res<ShadeDirtyRoots>,
    mut self_log: Local<f32>,
) {
    let Some(streamer) = streamer else {
        return;
    };
    let step = SHADE_RAMP_PER_SEC * time.delta_secs();
    let ambient_step = AMBIENT_RAMP_PER_SEC * time.delta_secs();
    for (root, gt, shade, is_self, root_vis) in &mut roots {
        // `WOW_INTERIOR_LOG=1`: the viewer's own node state every 3 s.
        if is_self.is_some() && time.elapsed_secs() - *self_log > 3.0 && interior_log_enabled() {
            let p = gt.translation();
            eprintln!(
                "[self-node] at wow ({:.1}, {:.1}, {:.1})  t {:.2} -> {:.2} (I {:.2})  indoor {} \
                 on_wmo {}",
                -p.z,
                -p.x,
                p.y,
                shade.t,
                shade.effective_target(),
                2.5 - 2.0 * shade.t,
                shade.indoor,
                shade.on_wmo,
            );
            *self_log = time.elapsed_secs();
        }
        let shade = shade.into_inner();
        let pos = gt.translation();
        if !shade.sampled || pos.distance_squared(shade.last_pos) >= RESAMPLE_DIST_SQ {
            match doodad_ground_shade(&streamer, &adt_tiles, pos) {
                ShadeResolve::Ready(shadowed) => {
                    shade.on_sample(shadowed, pos);
                }
                // The tile is still decoding: keep the last state and retry next frame.
                ShadeResolve::Pending => {}
            }
        }
        let target = shade.effective_target();
        // `WOW_INTERIOR_LOG=1`: a line whenever a node's target moves.
        if (target - shade.logged_target).abs() > f32::EPSILON && interior_log_enabled() {
            eprintln!(
                "[node] root {root:?} at ({:.1}, {:.1}, {:.1}) -> target t {target:.2} \
                 (I {:.2}, indoor {}) from t {:.2}",
                pos.x,
                pos.y,
                pos.z,
                2.5 - 2.0 * target,
                shade.indoor,
                shade.t,
            );
            shade.logged_target = target;
        }
        shade.step_intensity(step);
        let a = shade.ambient;
        let at = shade.ambient_target;
        shade.ambient = Vec3::new(
            ramp_toward(a.x, at.x, ambient_step),
            ramp_toward(a.y, at.y, ambient_step),
            ramp_toward(a.z, at.z, ambient_step),
        );
        let byte = (shade.t * 255.0).round().clamp(0.0, 255.0) as u8;
        // Before the hidden skip: a hidden body's card must still find its root here.
        if root_shade.get(&root) != Some(&byte) {
            root_shade.insert(root, byte);
        }
        // A hidden root skips the walk; its ramps kept stepping, and the cleared `asserted` makes
        // the wake frame's walk unconditional.
        if root_vis.is_some_and(|v| *v == Visibility::Hidden) {
            shade.asserted = None;
            continue;
        }
        // Settled: every part already carries this byte and no other lane rewrote a tag since.
        if shade.asserted == Some(byte) && !dirty_roots.0.contains(&root) {
            continue;
        }
        shade.asserted = Some(byte);
        // Every part below the root, held items on deeper joints included, written on a change.
        for part in children.iter_descendants(root) {
            let Ok((mut tag, lit, own_probe, own_def)) = parts.get_mut(part) else {
                continue;
            };
            // A WMO doodad prop, lit by its own def, is in this tree only to ride a transport.
            if own_def {
                continue;
            }
            // `None`: this part's bits 6..=18 hold a probe slot.
            let Some(ext) = exterior_payload(lit.is_some_and(InteriorLit::is_bake), own_probe)
            else {
                continue;
            };
            if shade_of(tag.0, ext) != byte {
                tag.0 = with_shade(tag.0, byte, ext);
            }
        }
    }
    // The cards: the reference shades every batch of an object through one node, so a card takes
    // the nearest shaded root above its owner (on a mounted unit, the mount's).
    for (card, mut tag, lit, own_probe, own_def) in &mut cards {
        // As in the walk: walking up, a prop's card would reach a host that does not light it.
        if own_def {
            continue;
        }
        let Some(ext) = exterior_payload(lit.is_some_and(InteriorLit::is_bake), own_probe) else {
            continue;
        };
        let Some(byte) = card_root_shade(&root_shade, &child_of, card.follows()) else {
            continue; // a fixed terrain doodad's card: its shade rides the material selector
        };
        if shade_of(tag.0, ext) != byte {
            tag.0 = with_shade(tag.0, byte, ext);
        }
    }
}

/// The byte of the nearest shaded root at or above `follows`, the owner itself first.
fn card_root_shade(
    root_shade: &bevy::ecs::entity::EntityHashMap<u8>,
    child_of: &Query<&ChildOf>,
    follows: Option<Entity>,
) -> Option<u8> {
    let mut node = follows?;
    loop {
        if let Some(&byte) = root_shade.get(&node) {
            return Some(byte);
        }
        node = child_of.get(node).ok()?.parent();
    }
}

/// `WOW_INTERIOR_LOG=1`: the interior and shade instrument lines, read once.
fn interior_log_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_INTERIOR_LOG").is_some())
}

/// One linear step toward a target, never past it, as the reference's chase.
fn ramp_toward(v: f32, target: f32, step: f32) -> f32 {
    if v < target {
        (v + step).min(target)
    } else {
        (v - step).max(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::entity::EntityHashMap;
    use bevy::ecs::system::SystemState;

    /// Run the card resolve against a world's live `ChildOf` topology.
    fn resolve(world: &mut World, map: &EntityHashMap<u8>, follows: Option<Entity>) -> Option<u8> {
        let mut state: SystemState<Query<&ChildOf>> = SystemState::new(world);
        card_root_shade(map, &state.get(world), follows)
    }

    #[test]
    fn a_foreign_tag_write_flags_exactly_its_root() {
        let mut app = App::new();
        app.init_resource::<ShadeDirtyRoots>()
            .add_systems(Update, detect_shade_reclaims);
        let root = app.world_mut().spawn(GroundShade::default()).id();
        let part = app.world_mut().spawn((MeshTag(7), ChildOf(root))).id();
        let other = app.world_mut().spawn(GroundShade::default()).id();
        app.world_mut().spawn((MeshTag(9), ChildOf(other)));

        app.update(); // both parts are Added ⇒ both roots flagged
        let dirty = |app: &App| {
            let d = app.world().resource::<ShadeDirtyRoots>();
            (d.0.contains(&root), d.0.contains(&other))
        };
        assert_eq!(dirty(&app), (true, true), "a fresh part flags its root");

        app.update(); // nobody wrote ⇒ quiet
        assert_eq!(dirty(&app), (false, false), "a settled frame flags nothing");

        app.world_mut().get_mut::<MeshTag>(part).unwrap().0 = 42;
        app.update();
        assert_eq!(
            dirty(&app),
            (true, false),
            "a foreign write flags its own root and nobody else's"
        );
    }

    #[test]
    fn a_card_takes_its_roots_byte_from_any_depth() {
        let mut world = World::new();
        let root = world.spawn_empty().id();
        let a = world.spawn(ChildOf(root)).id();
        let b = world.spawn(ChildOf(a)).id();
        let c = world.spawn(ChildOf(b)).id();
        let mut map = EntityHashMap::default();
        map.insert(root, 7);
        assert_eq!(resolve(&mut world, &map, Some(c)), Some(7), "3 deep");
        assert_eq!(
            resolve(&mut world, &map, Some(root)),
            Some(7),
            "the root itself"
        );
    }

    #[test]
    fn an_unshaded_card_is_skipped() {
        let mut world = World::new();
        let stray_root = world.spawn_empty().id();
        let stray = world.spawn(ChildOf(stray_root)).id();
        let mut map = EntityHashMap::default();
        map.insert(world.spawn_empty().id(), 9);
        assert_eq!(resolve(&mut world, &map, None), None, "follows nothing");
        assert_eq!(
            resolve(&mut world, &map, Some(stray)),
            None,
            "no shaded ancestor"
        );
    }

    /// A tag holds a probe slot on the classifier's Bake law, or from spawn on a WMO doodad prop,
    /// which carries no `InteriorLit`.
    #[test]
    fn either_probe_population_denies_the_shade_writer_its_witness() {
        assert!(
            exterior_payload(false, false).is_some(),
            "an ordinary exterior part takes the shade byte"
        );
        assert!(
            exterior_payload(true, false).is_none(),
            "the classifier's Bake law owns the payload"
        );
        assert!(
            exterior_payload(false, true).is_none(),
            "a spawn-time MODD prop owns it too, with no InteriorLit to say so"
        );
        assert!(
            exterior_payload(true, true).is_none(),
            "and either alone is enough — the question is an OR, not a tie-break"
        );
    }

    /// A mounted unit: the mount's shade root is a descendant of the rider's.
    #[test]
    fn a_nested_root_wins_by_nearness() {
        let mut world = World::new();
        let rider = world.spawn_empty().id();
        let mount = world.spawn(ChildOf(rider)).id();
        let mount_joint = world.spawn(ChildOf(mount)).id();
        let mut map = EntityHashMap::default();
        map.insert(rider, 3);
        map.insert(mount, 9);
        assert_eq!(
            resolve(&mut world, &map, Some(mount_joint)),
            Some(9),
            "the MOUNT's byte"
        );
        assert_eq!(resolve(&mut world, &map, Some(mount)), Some(9));
    }

    /// A new node holds intensity 1.0, takes its first sample's target, and steps toward it at
    /// 3.3333333 x dt without snapping (`0x670e75`, `0x69e770`, `0x69e880`).
    #[test]
    fn a_new_node_ramps_from_1_to_its_target() {
        let mut n = GroundShade::default();
        assert_eq!(n.intensity(), 1.0, "created at 1.0");
        n.step_intensity(SHADE_RAMP_PER_SEC * 0.1);
        assert_eq!(n.intensity(), 1.0, "unsampled: target 1.0 holds it");
        n.on_sample(false, Vec3::ZERO);
        let dt = 1.0 / 60.0;
        n.step_intensity(SHADE_RAMP_PER_SEC * dt);
        let want = (1.0 + 3.333_333_3 * dt).min(2.5);
        assert!(
            (n.intensity() - want).abs() < 1e-5,
            "{} vs {want}",
            n.intensity()
        );
        assert!(n.intensity() < 2.5, "no snap to the target");
        for _ in 0..60 {
            n.step_intensity(SHADE_RAMP_PER_SEC * dt);
        }
        assert_eq!(n.intensity(), 2.5, "settles on the lit target");
    }

    /// The three targets: 2.5 lit, 0.5 in the chunk's shadow, 1.0 as an interior.
    #[test]
    fn the_targets_are_2_5_lit_0_5_shadowed_1_interior() {
        let at = |n: &GroundShade| 2.5 - 2.0 * n.effective_target();
        let mut n = GroundShade::default();
        n.on_sample(false, Vec3::ZERO);
        assert_eq!(at(&n), 2.5);
        n.on_sample(true, Vec3::ZERO);
        assert_eq!(at(&n), 0.5);
        n.indoor = true;
        assert_eq!(at(&n), 1.0);
    }

    /// The entity family's intensity reaches the SH unclamped (`0x71c4e0`): the shader's lit
    /// entity expression carries no `min(I, 1)`.
    #[test]
    fn the_entity_intensity_is_not_capped_in_the_shader() {
        let src = include_str!("../../benilla-assets/src/shaders/wow_model.wgsl");
        let line = src
            .lines()
            .find(|l| l.trim_start().starts_with("let intensity ="))
            .expect("the intensity line");
        assert!(line.contains("mix(2.5, 0.5, shade_t)"), "{line}");
        assert!(
            !line.contains("min("),
            "a lit entity packs above 1.0: {line}"
        );
    }
}
