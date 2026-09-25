//! Interior lighting for entity M2s (units, players, GameObjects, held items): which stand in a
//! WMO room, and the light law they take there.
//!
//! One law lights every entity M2 (`Node::SetModel` `0x6716f0`, dispatched `0x672a20`): indoors,
//! the node's attach rays onto the WMO render mesh and bakes the hit's barycentric MOCV
//! (floor-168 / cap-96) on the fixed interior axis, plus the hit group's MOLR lobes, folded here
//! into an SH probe. Indoors is the lighting class (`[node+0xc]`: outdoor iff `MOGI & 0x48`), not
//! the zone-text bit, which calls `0x40`-only streets indoors. One verdict per unit, at its anchor.

use std::collections::HashSet;

use benilla_assets::{cap96, floor168, AdtTile, WmoModel};
use bevy::asset::AssetId;
use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::relationship::RelationshipTarget as _;
use bevy::ecs::world::DeferredWorld;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::lighting::{PropProbeSlot, PropProbes};
use crate::model_fade::{PendingAppearFade, RenderFade};
use crate::terrain_stream::{
    fold_interior_probe, interior_light_up, PropLobeLight, TerrainStreamer,
};
use crate::wmo_portal::{indoor_verdict_at, IndoorVerdict, WmoPortalInstance};
use benilla_assets::materials::WowModelMaterial;

/// Squared distance (yd²) before a re-test: an epsilon, since the reference re-runs a unit's
/// classify and footprint chain every tick (`0x69e280`), so the MOCV light must never step.
const RESAMPLE_DIST_SQ: f32 = 1e-4;

/// The resident WMO placements, as a change signal: a building streaming in under a standing NPC
/// must re-light it. Rebuilt each frame by [`crate::terrain_stream`].
#[derive(Resource, Default)]
pub struct WmoResidency {
    resident: HashSet<AssetId<WmoModel>>,
    generation: u32,
}

impl WmoResidency {
    /// Bumped whenever the resident set changes.
    pub(crate) fn generation(&self) -> u32 {
        self.generation
    }

    /// Bumps the generation only on a real change, so the per-frame rebuild keeps anchors settled.
    pub(crate) fn update(&mut self, next: impl IntoIterator<Item = AssetId<WmoModel>>) {
        let next_ids: HashSet<AssetId<WmoModel>> = next.into_iter().collect();
        if next_ids != self.resident {
            self.resident = next_ids;
            self.generation = self.generation.wrapping_add(1);
        }
    }
}

/// The indoor law a part takes, fixed at build time by whether it built a bake variant.
#[derive(Clone)]
pub(crate) enum InteriorKind {
    /// A part with no bake variant: indoors it keeps the day/night matte at gain 1.0.
    Matte,
    /// The footprint-MOCV bake: `material` is the prop-lane variant, which evaluates the SH probe
    /// the `MeshTag` slot names, and `center` the M2 vertex-box centre (model-local), the fold's
    /// MOLR reference point.
    Bake {
        material: Handle<WowModelMaterial>,
        center: Vec3,
    },
}

/// One M2 batch's interior-light membership. Every batch of a model takes it, billboard cards
/// included: the reference shades every batch through the object's one node (`0x7192b0`), and a
/// billboard bone turns geometry, not light. `anchor` is the model's net entity root, so a model
/// never splits across laws; `None` is a WMO-display part, with no interior variant. The caller
/// seeds the part's [`MeshTag`](bevy::mesh::MeshTag) (rig slot, fade alpha); the classifier
/// composes into it.
pub fn part_interior_lit(
    exterior: &Handle<WowModelMaterial>,
    interior: Option<&Handle<WowModelMaterial>>,
    bake: Option<&Handle<WowModelMaterial>>,
    center: Vec3,
    anchor: Entity,
) -> Option<(InteriorLit, ClassifiedBy)> {
    interior?;
    let kind = match bake {
        Some(material) => InteriorKind::Bake {
            material: material.clone(),
            center,
        },
        None => InteriorKind::Matte,
    };
    Some((
        InteriorLit::new(kind, exterior.clone()),
        ClassifiedBy(anchor),
    ))
}

/// The law a part renders under.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AppliedLaw {
    Exterior,
    /// Indoors on the day/night pair, unmodified, at gain 1.0: the reference's null-node fallback
    /// (`0x672a2f`), for a footprint ray that missed or hit a MOPY&1 face, or a bake-less part.
    Matte,
    /// Indoors on the footprint bake, evaluated from this probe-table slot.
    Bake(u16),
}

/// The body model's vertex-box centre (model-local) on the net entity root: the MOLR reference
/// point for every part under the root, held items included, since an item aliases its wearer's
/// light collector (`[item+0x3b8]=[wearer+0x3b8]`, `0x718960`). For a [`ContainmentAttach`]
/// anchor it is also the attach point, the reference's `[node+0x5c]`.
#[derive(Component, Clone, Copy)]
pub struct BodyBakeCenter(pub Vec3);

/// This anchor's node takes the containment attach from its bounding-box centre (`0x6a8c10`), not
/// the down-ray from its position (`0x6a8a20`): `[node+0x90]` bit 13, set from the TYPEMASK at node
/// creation (`0x613e10`/`0x670db0`) for GameObjects alone, forked at `0x6a86d0`. A GameObject's
/// origin often sits at or under its own floor.
#[derive(Component)]
pub struct ContainmentAttach;

/// A part's net entity root, its anchor; bevy keeps the anchor's [`LitParts`] in step.
#[derive(Component)]
#[relationship(relationship_target = LitParts)]
pub struct ClassifiedBy(pub Entity);

/// An anchor's parts, maintained by [`ClassifiedBy`]; never edited by hand.
#[derive(Component)]
#[relationship_target(relationship = ClassifiedBy)]
pub struct LitParts(Vec<Entity>);

/// A lit particle emitter's anchor, the net entity root whose light node it draws under. The
/// reference builds the node from the object's type (`0x613e10` → `0x670db0` → `0x7134b0`) and
/// commits its light to a particle draw as to a submesh (`0x70baf0`, the material synthesized per
/// draw at `0x70d8b0`), so an object with only unlit mesh batches is classified for its emitters.
#[derive(Component)]
#[relationship(relationship_target = LitEmitters)]
pub struct EmitterLitBy(pub Entity);

/// An anchor's lit emitters, maintained by [`EmitterLitBy`]; never edited by hand.
#[derive(Component)]
#[relationship_target(relationship = EmitterLitBy)]
pub struct LitEmitters(Vec<Entity>);

/// The constant a lit particle of this anchor multiplies by: the anchor's whole committed term
/// (`ambient + 0.9·diffuse + Σ lamps`) along world up, a particle quad's only normal (one constant
/// per draw, `0x7b3fd0`, `0x58b0b0`). Present only on the Bake law; absent, the scene's light.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct ParticleLight(pub [f32; 3]);

/// The ambient word alone (`GroundShade::ambient`, ramping toward `cap96(MOCV)`), for a draw whose
/// vertex format has no normal and so evaluates only `Σ ambient × colour` (`0x592a60`): the weapon
/// trail, lit by its wearer's node (`0x70d982` → `0x70ca50` → `0x70baf0`). Present only on the Bake
/// law; outdoors the day/night ambient applies (`0x69e4ad`).
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct NodeAmbient(pub [f32; 3]);

/// An anchor's classification: its parts' law and room, and the movement/residency re-test gate.
#[derive(Component)]
pub struct InteriorAnchor {
    law: AppliedLaw,
    /// The attach's room, `None` outside; kept while settled, as its fog gate tracks the camera.
    room: Option<crate::wmo_portal::WmoRoom>,
    /// That room's fog verdict, re-read every frame ([`anchor_room_fogged`]).
    fog: bool,
    /// The position and residency generation at the last ray: the re-test gate.
    last_pos: Vec3,
    generation: u32,
    /// Resolved from a bake-capable part; a bake part joining a matte-resolved anchor re-resolves.
    kind_bake: bool,
}

impl InteriorAnchor {
    /// This anchor's law, for the inspect card's light readout (`crate::interact`).
    pub fn law_label(&self) -> String {
        match self.law {
            AppliedLaw::Exterior => "exterior".into(),
            AppliedLaw::Matte => "interior day/night".into(),
            AppliedLaw::Bake(slot) => format!("interior bake (probe {slot})"),
        }
    }
}

/// Parts to re-author from their anchor's law, drained every run: a part joining a settled
/// anchor, a fade latch ([`enqueue_on_fade_latch`]), the self-avatar zoom feather's release.
#[derive(Resource, Default)]
pub struct InteriorReauthor(pub Vec<Entity>);

/// One M2 entity part's material variants, which [`classify_entity_interior`] swaps; adding it
/// queues the part, so a part joining a settled anchor still takes the standing law.
#[derive(Component)]
#[component(on_add = enqueue_new_part)]
pub struct InteriorLit {
    kind: InteriorKind,
    /// The exterior material, which the Matte law shares: day/night is the intensity byte at 1.0.
    exterior: Handle<WowModelMaterial>,
    /// The law last written, the write gate; the anchor's [`InteriorAnchor`] is the authority.
    applied: Option<AppliedLaw>,
    /// The [`crate::mesh_tag::INTERIOR_FOG_BIT`] last written; it follows the camera, not the law.
    fogged: bool,
}

impl InteriorLit {
    /// On the Bake law: the tag carries the probe slot, so [`crate::entity_shade`] must not write
    /// the shade byte over it.
    pub(crate) fn is_bake(&self) -> bool {
        matches!(self.applied, Some(AppliedLaw::Bake(_)))
    }

    /// The steady material for the current law, which the fade writers settle onto: the bake
    /// variant on the Bake law, else the exterior one (a matte-kind part has no bake variant).
    pub(crate) fn steady_material(&self) -> &Handle<WowModelMaterial> {
        match (self.applied, &self.kind) {
            (Some(AppliedLaw::Bake(_)), InteriorKind::Bake { material, .. }) => material,
            _ => &self.exterior,
        }
    }

    /// Re-points the variants at a re-dressed material set (`entities::attach::redress`) and
    /// returns what to draw; the applied law stays, since a re-dress moves nothing.
    pub fn repoint(
        &mut self,
        exterior: &Handle<WowModelMaterial>,
        bake: Option<&Handle<WowModelMaterial>>,
    ) -> &Handle<WowModelMaterial> {
        self.exterior = exterior.clone();
        if let (InteriorKind::Bake { material, .. }, Some(b)) = (&mut self.kind, bake) {
            *material = b.clone();
        }
        self.steady_material()
    }

    pub(crate) fn new(kind: InteriorKind, exterior: Handle<WowModelMaterial>) -> Self {
        Self {
            kind,
            exterior,
            applied: None,
            fogged: false,
        }
    }

    /// Test-only: a part already on the Bake law, for tests outside this module.
    #[cfg(test)]
    pub(crate) fn applied_bake_for_test(
        kind: InteriorKind,
        exterior: Handle<WowModelMaterial>,
    ) -> Self {
        Self {
            kind,
            exterior,
            applied: Some(AppliedLaw::Bake(0)),
            fogged: true,
        }
    }
}

/// Queues a new part: one joining a settled anchor would get no law-change write otherwise.
fn enqueue_new_part(mut world: DeferredWorld, ctx: HookContext) {
    if let Some(mut queue) = world.get_resource_mut::<InteriorReauthor>() {
        queue.0.push(ctx.entity);
    }
}

/// Queues a part whose `RenderFade` leaves. The re-author is normally a no-op, kept as the
/// backstop against a part left on a freed probe slot, which renders black.
fn enqueue_on_fade_latch(
    fade_end: On<Remove, RenderFade>,
    parts: Query<(), With<InteriorLit>>,
    mut queue: ResMut<InteriorReauthor>,
) {
    if parts.contains(fade_end.entity) {
        queue.0.push(fade_end.entity);
    }
}

/// The residency registry, the reauthor queue with its fade-latch observer, and the classifier.
pub(crate) struct InteriorPlugin;

impl Plugin for InteriorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WmoResidency>()
            .init_resource::<InteriorReauthor>()
            // After the room flood: the fog half of a part's channel is the flood's `[0xca7f00]`
            // chain, and before it a camera move would read last frame's rooms.
            .add_systems(
                Update,
                classify_entity_interior
                    .after(crate::wmo_portal::WmoPvsSet)
                    // The fold reads the resolved `WowLighting`.
                    .in_set(crate::lighting::LightingConsumeSet),
            )
            .add_observer(enqueue_on_fade_latch);
    }
}

/// The bake fold's ray products on the anchor, refolded without a new ray while the node's ramps
/// still move and the entity does not.
#[derive(Component)]
pub struct BakeState {
    /// floor-168 of the footprint MOCV (0..1): the fold's diffuse, before the node intensity.
    word: Vec3,
    /// The hit group's windowed MOLR lobes (world space, pre-gained).
    lobes: Vec<PropLobeLight>,
    /// The fold's MOLR reference point (world space) at the last ray.
    ref_point: Vec3,
}

/// Lights each entity part by where its anchor stands: outside, the global SH × the ramped
/// intensity byte; in a room, the footprint bake folded into the anchor's own SH probe, or the
/// day/night matte. An anchor rays again only when it moves or a building streams; its parts are
/// written only when its law changes, or through [`InteriorReauthor`].
#[allow(clippy::type_complexity)]
pub fn classify_entity_interior(
    mut commands: Commands,
    time: Res<Time>,
    residency: Res<WmoResidency>,
    wmos: Res<Assets<WmoModel>>,
    instances: Query<(Entity, &WmoPortalInstance)>,
    streamer: Res<TerrainStreamer>,
    adt_tiles: Res<Assets<AdtTile>>,
    lighting: Res<crate::lighting::WowLighting>,
    mut probes: ResMut<PropProbes>,
    // Light nodes with a consumer, a lit mesh part or a lit emitter ([`EmitterLitBy`]): any other
    // node would still ray and take a slot from a bounded probe table.
    mut anchors: Query<
        (
            Entity,
            &GlobalTransform,
            Option<&LitParts>,
            Option<&mut InteriorAnchor>,
            Has<ContainmentAttach>,
            Option<&BodyBakeCenter>,
            Option<&LitEmitters>,
            Option<&mut ParticleLight>,
            Option<&mut NodeAmbient>,
        ),
        Or<(With<LitParts>, With<LitEmitters>)>,
    >,
    mut nodes: Query<&mut crate::entity_shade::GroundShade>,
    bake_states: Query<&BakeState>,
    seats: Query<&PropProbeSlot>,
    mut queue: ResMut<InteriorReauthor>,
    part_anchors: Query<&ClassifiedBy>,
    // Fading parts included, or an entity streaming in indoors fades in under exterior light. The
    // writes keep the tag's alpha, and take the law's blend twin by the ramp's own rule
    // (`FadeMaterials::material_for`), so the two writers never fight.
    mut parts: PartWrite,
) {
    let _t0 = std::time::Instant::now();
    let (mut n_anchors, mut n_resolved, mut n_written) = (0usize, 0usize, 0usize);
    // Anchors with no part to ask, the tripwire for a fade lockout creeping back.
    let mut n_fade_blocked = 0usize;
    let mut resolve_us = 0.0f32;
    for (
        anchor,
        anchor_t,
        lit_parts,
        mut state,
        containment,
        bake_center,
        lit_emitters,
        particle_light,
        node_ambient,
    ) in &mut anchors
    {
        n_anchors += 1;
        let pos = anchor_t.translation();
        let had_state = state.is_some();
        // A settled anchor skips the ray. On the Bake law it refolds from the cached products while
        // its ramps chase, so a unit that stops in a warm room does not freeze mid-ramp.
        if let Some(state) = state.as_deref_mut() {
            let settled = state.generation == residency.generation
                && pos.distance_squared(state.last_pos) < RESAMPLE_DIST_SQ;
            if settled {
                if let AppliedLaw::Bake(slot) = state.law {
                    if let (Ok(node), Ok(bake)) = (nodes.get(anchor), bake_states.get(anchor)) {
                        if !node.ramps_settled() {
                            let words = (
                                node.ambient.to_array(),
                                (bake.word * node.intensity()).to_array(),
                            );
                            probes.update_owned(
                                slot,
                                fold_interior_probe(words.0, words.1, bake.ref_point, &bake.lobes),
                            );
                            // The emitters' constant, from the same words through its own curve.
                            if let Some(mut light) = particle_light {
                                light.set_if_neq(ParticleLight(interior_light_up(
                                    words.0,
                                    words.1,
                                    bake.ref_point,
                                    &bake.lobes,
                                )));
                            }
                            // The ambient word alone, for normal-less draws ([`NodeAmbient`]).
                            if let Some(mut amb) = node_ambient {
                                amb.set_if_neq(NodeAmbient(words.0));
                            }
                        }
                    }
                }
                // The fog is not settled: its gate is the camera's chain, so a still unit leaves
                // and rejoins its building's MFOG as the camera walks between rooms.
                let fog = anchor_room_fogged(state.room, &instances);
                if state.fog != fog {
                    state.fog = fog;
                    n_written += write_anchor_parts(state.law, fog, lit_parts, &mut parts);
                }
                continue;
            }
        }
        // Re-resolving. The law's one input is the fold's reference point (`None` for the matte),
        // asked of the first part; an emitter-only anchor bakes from its own centre.
        let part_bake = lit_parts
            .into_iter()
            .flat_map(LitParts::iter)
            .find_map(|part| {
                parts.get(part).ok().map(|(lit, ..)| match &lit.kind {
                    InteriorKind::Bake { center, .. } => Some(*center),
                    InteriorKind::Matte => None,
                })
            });
        let bake = match part_bake {
            Some(center) => center,
            None if lit_emitters.is_some() => Some(bake_center.map_or(Vec3::ZERO, |c| c.0)),
            None => {
                n_fade_blocked += 1;
                continue;
            }
        };
        let seated = seats.get(anchor).ok().map(|s| s.0);
        n_resolved += 1;
        let _r = std::time::Instant::now();
        let (attach, attach_at) = attach_anchor(containment, bake_center, anchor_t);
        let (law, room) = resolve_anchor_law(
            &mut commands,
            &mut probes,
            &wmos,
            &instances,
            &streamer,
            &adt_tiles,
            &lighting,
            &mut nodes,
            anchor,
            anchor_t,
            attach,
            attach_at,
            bake,
            seated,
        );
        resolve_us += _r.elapsed().as_secs_f32() * 1e6;
        let fog = anchor_room_fogged(room, &instances);
        let kind_bake = bake.is_some();
        let changed = match state.as_deref_mut() {
            Some(state) => {
                let changed = state.law != law || state.fog != fog;
                state.law = law;
                state.room = room;
                state.fog = fog;
                state.last_pos = pos;
                state.generation = residency.generation;
                state.kind_bake = kind_bake;
                changed
            }
            None => {
                // `try_insert`: the anchor may carry a same-frame despawn already queued.
                commands.entity(anchor).try_insert(InteriorAnchor {
                    law,
                    room,
                    fog,
                    last_pos: pos,
                    generation: residency.generation,
                    kind_bake,
                });
                true
            }
        };
        // Write parts only on a change, so a unit moving mid-room does not churn extraction.
        if !changed {
            continue;
        }
        // `WOW_INTERIOR_LOG=1` prints interior verdicts and flips out of them, with the attach and
        // the point it probed; `=all` adds the first-resolve exteriors.
        let log = std::env::var("WOW_INTERIOR_LOG").ok();
        if log.is_some()
            && (law != AppliedLaw::Exterior || had_state || log.as_deref() == Some("all"))
        {
            // The part and emitter counts show what joined: compare the model's batch count
            // (`benilla-extract m2batch <model>`).
            let room = match room {
                Some(r) => format!(
                    " room g{} of {:?} · fog {}",
                    r.group,
                    r.instance,
                    if fog { "INTERIOR" } else { "scene (off-chain)" }
                ),
                None => String::new(),
            };
            eprintln!(
                "[interior] t {:.2} anchor {anchor:?} ({} parts, {} emitters) at \
                 ({:.1}, {:.1}, {:.1}) {} probe ({:.1}, {:.1}, {:.1}) -> {}{room}",
                time.elapsed_secs(),
                lit_parts.map_or(0, |p| p.len()),
                lit_emitters.map_or(0, |e| e.len()),
                pos.x,
                pos.y,
                pos.z,
                match attach {
                    crate::wmo_portal::LightAttach::Containment => "containment",
                    crate::wmo_portal::LightAttach::DownRay => "down-ray",
                },
                attach_at.x,
                attach_at.y,
                attach_at.z,
                match law {
                    AppliedLaw::Exterior => "exterior".to_string(),
                    AppliedLaw::Matte => "INTERIOR matte".to_string(),
                    AppliedLaw::Bake(s) => format!("INTERIOR bake slot {s}"),
                }
            );
        }
        n_written += write_anchor_parts(law, fog, lit_parts, &mut parts);
    }
    // Drain the queue past each part's change gate: a transient writer (a fade, the zoom
    // feather) may have overwritten its material or tag while `applied` stayed current.
    for part in std::mem::take(&mut queue.0) {
        let Ok(edge) = part_anchors.get(part) else {
            continue; // despawned since enqueue
        };
        let Ok((_, _, _, mut state, ..)) = anchors.get_mut(edge.0) else {
            continue;
        };
        let Some(state) = state.as_deref_mut() else {
            continue; // unresolved: the anchor's first resolve writes every part
        };
        let Ok((mut lit, mut material, mut tag, fm, ramping, pending)) = parts.get_mut(part) else {
            continue; // despawned between the enqueue and the drain
        };
        let fade = fm.filter(|_| ramping || pending);
        // A bake part on a matte-resolved anchor drops the record, so the next run re-rays; it
        // still takes the standing law this frame.
        if matches!(lit.kind, InteriorKind::Bake { .. }) && !state.kind_bake {
            commands.entity(edge.0).try_remove::<InteriorAnchor>();
        }
        n_written += usize::from(write_part_law(
            state.law,
            state.fog,
            &mut lit,
            &mut material,
            &mut tag,
            true,
            fade,
        ));
    }
    // `WOW_INTERIOR_COST=1`: anchors visited and resolved, parts written, and the cost per frame.
    static COST: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *COST.get_or_init(|| std::env::var_os("WOW_INTERIOR_COST").is_some()) {
        // `WOW_COLUMN_COST=1` adds this lane's column-grid queries; `spanning` (oversized floor
        // slabs tested on every query) should read near zero.
        let column = if benilla_assets::column_grid::column_cost_enabled() {
            let (queries, binned, coarse, spanning) =
                benilla_assets::column_grid::take_column_query_stats();
            let tested = binned + coarse + spanning;
            let pct = if tested == 0 {
                0.0
            } else {
                100.0 * spanning as f32 / tested as f32
            };
            format!(
                " col_queries={queries} col_tested={tested} col_binned={binned} col_coarse={coarse} col_spanning={spanning} spanning_pct={pct:.1}"
            )
        } else {
            String::new()
        };
        eprintln!(
            "[interior-cost] anchors={n_anchors} resolved={n_resolved} fade_blocked={n_fade_blocked} parts_written={n_written} resolve_ms={:.2} total_ms={:.2}{column}",
            resolve_us / 1000.0,
            _t0.elapsed().as_secs_f32() * 1000.0
        );
    }
}

/// The classifier's part-write query: the law record, the material and tag the law writes, and the
/// fade state that picks the blend twin.
type PartWrite<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut InteriorLit,
        &'static mut MeshMaterial3d<WowModelMaterial>,
        &'static mut MeshTag,
        Option<&'static crate::model_fade::FadeMaterials>,
        Has<RenderFade>,
        Has<PendingAppearFade>,
    ),
>;

/// Writes every part of one anchor from `(law, fog)`: the one loop the resolve and the fog gate
/// share, so they never write the channel differently.
fn write_anchor_parts(
    law: AppliedLaw,
    fog: bool,
    lit_parts: Option<&LitParts>,
    parts: &mut PartWrite,
) -> usize {
    let mut written = 0;
    for part in lit_parts.into_iter().flat_map(LitParts::iter) {
        if let Ok((mut lit, mut material, mut tag, fm, ramping, pending)) = parts.get_mut(part) {
            let fade = fm.filter(|_| ramping || pending);
            written += usize::from(write_part_law(
                law,
                fog,
                &mut lit,
                &mut material,
                &mut tag,
                false,
                fade,
            ));
        }
    }
    written
}

/// Writes one part's material and tag for `law`, gated on its record unless `force`d (a transient
/// writer overwrote the channel); returns whether it wrote. Material and tag must always name the
/// same law, mid-fade too, or the shader reads a Bake tag's probe slot (bits 6..=18) as a shade
/// byte. Other laws reset to the exterior payload, whose shade byte `entity_shade` re-asserts
/// after this; the alpha and rig fields carry through. `fog` needs both the unit's interior class
/// (`[node+0xc]&2`) and its room on the camera's `[0xca7f00]` chain (`[P+0x98]`); `fade` picks the
/// law's blend twin by the ramp's own rule ([`crate::model_fade::FadeMaterials::material_for`]).
fn write_part_law(
    law: AppliedLaw,
    fog: bool,
    lit: &mut InteriorLit,
    material: &mut MeshMaterial3d<WowModelMaterial>,
    tag: &mut MeshTag,
    force: bool,
    fade: Option<&crate::model_fade::FadeMaterials>,
) -> bool {
    if lit.applied == Some(law) && lit.fogged == fog && !force {
        return false;
    }
    lit.applied = Some(law);
    lit.fogged = fog;
    let want = match fade {
        Some(fm) => fm.material_for(Some(&*lit), true).clone(),
        None => lit.steady_material().clone(),
    };
    if material.0 != want {
        material.0 = want;
    }
    let payload = match law {
        AppliedLaw::Bake(slot) => crate::mesh_tag::with_interior_probe(tag.0, slot),
        AppliedLaw::Matte | AppliedLaw::Exterior => crate::mesh_tag::with_exterior_reset(tag.0),
    };
    tag.0 = crate::mesh_tag::with_interior_fog(payload, fog);
    true
}

/// Whether this anchor's room is on the camera's interior-fog chain this frame (`[P+0x98]`). It
/// fails closed at every seam, as `fogs_group` does: a miss must not paint a building's MFOG.
fn anchor_room_fogged(
    room: Option<crate::wmo_portal::WmoRoom>,
    instances: &Query<(Entity, &WmoPortalInstance)>,
) -> bool {
    let Some(room) = room else {
        return false;
    };
    let Ok((_, inst)) = instances.get(room.instance) else {
        return false;
    };
    inst.fogs_group(room.group)
}

/// The attach and its point, one fork at `0x6a86d0`: containment from the world bounding-box
/// centre (`[node+0x5c]`) for a GameObject, else the down-ray from the position (`[node+0xa8]`).
fn attach_anchor(
    containment: bool,
    bake_center: Option<&BodyBakeCenter>,
    anchor_t: &GlobalTransform,
) -> (crate::wmo_portal::LightAttach, Vec3) {
    use crate::wmo_portal::LightAttach;
    match (containment, bake_center) {
        (true, Some(BodyBakeCenter(center))) => {
            (LightAttach::Containment, anchor_t.transform_point(*center))
        }
        // No bounds yet: the origin, the down-ray's own point.
        (true, None) => (LightAttach::Containment, anchor_t.translation()),
        (false, _) => (LightAttach::DownRay, anchor_t.translation()),
    }
}

/// Rays one anchor and resolves its law, updating its node and, on the Bake law, folding into the
/// anchor's own probe slot. `seated`, the anchor's live [`PropProbeSlot`], is the only authority
/// on that slot: a part's cached `Bake(slot)` is never believed, or a fresh part could free the
/// slot under its siblings and leave them black.
fn resolve_anchor_law(
    commands: &mut Commands,
    probes: &mut PropProbes,
    wmos: &Assets<WmoModel>,
    instances: &Query<(Entity, &WmoPortalInstance)>,
    streamer: &TerrainStreamer,
    adt_tiles: &Assets<AdtTile>,
    lighting: &crate::lighting::WowLighting,
    nodes: &mut Query<&mut crate::entity_shade::GroundShade>,
    anchor: Entity,
    anchor_t: &GlobalTransform,
    attach: crate::wmo_portal::LightAttach,
    attach_at: Vec3,
    bake_center: Option<Vec3>,
    seated: Option<u16>,
) -> (AppliedLaw, Option<crate::wmo_portal::WmoRoom>) {
    let (verdict, claimed) = indoor_verdict_at(
        wmos,
        instances.iter(),
        streamer,
        adt_tiles,
        attach_at,
        attach,
    );
    // An outdoor-class WMO surface (street, deck, porch) sets the node's skip-shadow bit: the lit
    // 2.5 target with no MCSH beneath, which `entity_shade` reads.
    let on_wmo = matches!(verdict, IndoorVerdict::OutdoorsOnWmo);
    if let Ok(mut node) = nodes.get_mut(anchor) {
        if node.on_wmo != on_wmo {
            node.on_wmo = on_wmo;
        }
    }
    let law = match bake_center {
        None => match verdict {
            IndoorVerdict::DayNight | IndoorVerdict::Baked { .. } => AppliedLaw::Matte,
            IndoorVerdict::Outdoors | IndoorVerdict::OutdoorsOnWmo => AppliedLaw::Exterior,
        },
        Some(center) => {
            match verdict {
                IndoorVerdict::Outdoors | IndoorVerdict::OutdoorsOnWmo => AppliedLaw::Exterior,
                IndoorVerdict::DayNight => AppliedLaw::Matte,
                IndoorVerdict::Baked { mocv, lobes } => {
                    // The ambient chases cap96(MOCV) at the node's 2.0/s ramp, seeded from the
                    // scene ambient on entry (the reference's `[+0x9c]` carries across the leg
                    // flip); the diffuse is floor-168(MOCV) × the ramped intensity, plus MOLR.
                    let ref_point = anchor_t.transform_point(center);
                    let word = Vec3::from_array(floor168(mocv));
                    let (ambient, intensity) = match nodes.get_mut(anchor) {
                        Ok(mut node) => {
                            let target = Vec3::from_array(cap96(mocv));
                            // Entry is "no seated slot": a part joining a seated anchor (a gear
                            // swap indoors) must not reseed the ramp.
                            if seated.is_none() {
                                node.seed_ambient(Vec3::from_array(lighting.ambient), target);
                            } else {
                                node.ambient_target = target;
                            }
                            (node.ambient, node.intensity())
                        }
                        // No `GroundShade` yet: the settled words.
                        Err(_) => (Vec3::from_array(cap96(mocv)), 1.0),
                    };
                    let words = (ambient.to_array(), (word * intensity).to_array());
                    let coeffs = fold_interior_probe(words.0, words.1, ref_point, &lobes);
                    let emitter_light = interior_light_up(words.0, words.1, ref_point, &lobes);
                    let slot = match seated {
                        // Staying in Bake: the owned slot is rewritten in place.
                        Some(slot) => {
                            probes.update_owned(slot, coeffs);
                            Some(slot)
                        }
                        None => probes.alloc_owned(coeffs),
                    };
                    match slot {
                        Some(slot) => {
                            // `try_insert`: a same-frame despawn may already be queued.
                            commands.entity(anchor).try_insert((
                                BakeState {
                                    word,
                                    lobes,
                                    ref_point,
                                },
                                // A particle draw takes the fixed-function curve, not the SH one.
                                ParticleLight(emitter_light),
                                NodeAmbient(words.0),
                            ));
                            AppliedLaw::Bake(slot)
                        }
                        None => {
                            let (live, peak) = probes.occupancy();
                            warn_once!(
                                "interior-prop probe table full (live {live}, peak {peak}); \
                                 indoor entities fall back to the day/night law"
                            );
                            AppliedLaw::Matte
                        }
                    }
                }
            }
        }
    };
    // `entity_shade` picks the intensity target from this: 2.5/0.5 by MCSH outdoors, 1.0 indoors.
    if let Ok(mut node) = nodes.get_mut(anchor) {
        let indoor = law != AppliedLaw::Exterior;
        if node.indoor != indoor {
            node.indoor = indoor;
        }
    }
    // The slot's lifecycle, judged by the seated state: entering Bake seats it, leaving removes it
    // with the fold cache, and its component's hook frees the slot.
    match (seated, law) {
        (Some(old), AppliedLaw::Bake(new)) if old == new => {}
        (_, AppliedLaw::Bake(new)) => seat_probe_slot(commands, anchor, new),
        (Some(_), _) => {
            // On a despawned anchor the hook already ran; the removes just must not panic.
            commands
                .entity(anchor)
                .try_remove::<PropProbeSlot>()
                .try_remove::<BakeState>()
                .try_remove::<ParticleLight>()
                .try_remove::<NodeAmbient>();
        }
        _ => {}
    }
    // Only an indoor law keeps its room, so the fog gate never fires on a unit on a porch.
    let room = claimed
        .filter(|_| law != AppliedLaw::Exterior)
        .map(|(instance, group)| crate::wmo_portal::WmoRoom {
            instance,
            group: group as u16,
        });
    (law, room)
}

/// Seats a Bake slot on `anchor` at apply time. A same-frame despawn may have applied first; the
/// slot's component then never lands and its hook never frees it, so it is released here, since
/// the pool never resets.
fn seat_probe_slot(commands: &mut Commands, anchor: Entity, new: u16) {
    commands.queue(
        move |world: &mut World| match world.get_entity_mut(anchor) {
            Ok(mut e) => {
                // One insert: the slot's `on_replace` hook frees the outgoing slot.
                e.insert(PropProbeSlot(new));
            }
            Err(_) => world.resource_mut::<PropProbes>().release(new),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classifier_world() -> World {
        let mut world = World::new();
        world.init_resource::<Time>();
        world.init_resource::<WmoResidency>();
        world.init_resource::<Assets<WmoModel>>();
        world.init_resource::<Assets<AdtTile>>();
        world.init_resource::<crate::lighting::WowLighting>();
        world.init_resource::<PropProbes>();
        world.init_resource::<TerrainStreamer>();
        world.init_resource::<InteriorReauthor>();
        world
    }

    /// A part recorded on a freed slot would draw its zeroed rows, a black unit; queued, it takes
    /// the anchor's live slot while nothing moves.
    #[test]
    fn a_stale_part_converges_to_the_anchors_standing_law() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = classifier_world();
        let coeffs = [Vec4::ZERO; 7];

        // The anchor's live slot, and a freed one its part still remembers.
        let live = world
            .resource_mut::<PropProbes>()
            .alloc_owned(coeffs)
            .unwrap();
        let stale = world
            .resource_mut::<PropProbes>()
            .alloc_owned(coeffs)
            .unwrap();
        world.resource_mut::<PropProbes>().release(stale);

        let generation = world.resource::<WmoResidency>().generation;
        // A room whose gate is on, so the tag is the whole Bake payload `probe_bits` names.
        let instance = placement_with_gate(&mut world, true);
        let anchor = world
            .spawn((
                GlobalTransform::default(),
                PropProbeSlot(live),
                InteriorAnchor {
                    law: AppliedLaw::Bake(live),
                    room: Some(crate::wmo_portal::WmoRoom { instance, group: 0 }),
                    fog: true,
                    last_pos: Vec3::ZERO, // matches the transform: settled, no movement
                    generation,
                    kind_bake: true,
                },
            ))
            .id();
        let mut lit = InteriorLit::new(
            InteriorKind::Bake {
                material: Handle::default(),
                center: Vec3::ZERO,
            },
            Handle::default(),
        );
        lit.applied = Some(AppliedLaw::Bake(stale));
        let part = world
            .spawn((
                lit,
                ClassifiedBy(anchor),
                MeshMaterial3d::<WowModelMaterial>(Handle::default()),
                MeshTag(crate::mesh_tag::probe_bits(stale)),
            ))
            .id();

        world.run_system_once(classify_entity_interior).unwrap();

        let lit = world.get::<InteriorLit>(part).unwrap();
        assert!(
            matches!(lit.applied, Some(AppliedLaw::Bake(s)) if s == live),
            "the part's law re-anchors on the anchor's standing slot"
        );
        assert_eq!(
            world.get::<MeshTag>(part).unwrap().0,
            crate::mesh_tag::probe_bits(live),
            "the tag reads the anchor's live slot, not the freed (black) one"
        );
        let (occupancy, _) = world.resource::<PropProbes>().occupancy();
        assert_eq!(
            occupancy, 1,
            "repair neither re-allocates nor frees the live slot"
        );
    }

    /// Onyxia's lava traps have this shape: all five mesh batches unlit (flags 0x13), three lit
    /// emitters.
    #[test]
    fn an_object_whose_only_lit_consumer_is_an_emitter_is_still_classified() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = classifier_world();
        // A GameObject: no mesh part joins the registry, one lit emitter does.
        let trap = world
            .spawn((
                GlobalTransform::default(),
                ContainmentAttach,
                BodyBakeCenter(Vec3::new(0.0, 1.0, 0.0)),
            ))
            .id();
        world.spawn(EmitterLitBy(trap));
        // The control: emitters, none lit, so nothing registers.
        let unlit = world.spawn(GlobalTransform::default()).id();

        world.run_system_once(classify_entity_interior).unwrap();

        let state = world
            .get::<InteriorAnchor>(trap)
            .expect("a light node with a lit emitter is classified even with no lit mesh part");
        assert!(
            state.kind_bake,
            "with no part to ask, the node takes the footprint law from its own bake centre"
        );
        assert!(
            world.get::<InteriorAnchor>(unlit).is_none(),
            "a node with no lit consumer is never visited"
        );
    }

    /// A one-group placement whose interior-fog gate (the `[0xca7f00]` chain) is `gate`.
    fn placement_with_gate(world: &mut World, gate: bool) -> Entity {
        world
            .spawn(WmoPortalInstance {
                handle: Handle::default(),
                world_from_local: bevy::math::Affine3A::IDENTITY,
                name_set: 0,
                visible: vec![true],
                interior_fog: vec![gate],
                liquid_visited: vec![false],
                flooded: vec![None],
            })
            .id()
    }

    /// A gear swap indoors: no law change and no movement.
    #[test]
    fn a_fresh_part_on_a_settled_anchor_takes_the_standing_law() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = classifier_world();
        let generation = world.resource::<WmoResidency>().generation;
        let instance = placement_with_gate(&mut world, true);
        let anchor = world
            .spawn((
                GlobalTransform::default(),
                InteriorAnchor {
                    law: AppliedLaw::Matte,
                    room: Some(crate::wmo_portal::WmoRoom { instance, group: 0 }),
                    fog: true,
                    last_pos: Vec3::ZERO,
                    generation,
                    kind_bake: false,
                },
            ))
            .id();
        let part = world
            .spawn((
                InteriorLit::new(InteriorKind::Matte, Handle::default()),
                ClassifiedBy(anchor),
                MeshMaterial3d::<WowModelMaterial>(Handle::default()),
                MeshTag(0),
            ))
            .id();

        world.run_system_once(classify_entity_interior).unwrap();

        let lit = world.get::<InteriorLit>(part).unwrap();
        assert_eq!(lit.applied, Some(AppliedLaw::Matte));
        assert_ne!(
            world.get::<MeshTag>(part).unwrap().0 & crate::mesh_tag::INTERIOR_FOG_BIT,
            0,
            "the day/night law in a room whose gate is on carries the room's fog bit"
        );
    }

    /// The `[P+0x98]` conjunct: when the camera, not the unit, takes the room off the chain, the
    /// unit returns to the scene fog with its light law unchanged.
    #[test]
    fn a_settled_anchor_follows_its_rooms_fog_gate_without_moving() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = classifier_world();
        let generation = world.resource::<WmoResidency>().generation;
        let instance = placement_with_gate(&mut world, true);
        let anchor = world
            .spawn((
                GlobalTransform::default(),
                InteriorAnchor {
                    law: AppliedLaw::Matte,
                    room: Some(crate::wmo_portal::WmoRoom { instance, group: 0 }),
                    fog: true,
                    last_pos: Vec3::ZERO, // settled: the walk never re-rays this anchor
                    generation,
                    kind_bake: false,
                },
            ))
            .id();
        let part = world
            .spawn((
                InteriorLit::new(InteriorKind::Matte, Handle::default()),
                ClassifiedBy(anchor),
                MeshMaterial3d::<WowModelMaterial>(Handle::default()),
                MeshTag(0),
            ))
            .id();
        world.run_system_once(classify_entity_interior).unwrap();
        let fog_of = |world: &World, part| {
            world.get::<MeshTag>(part).unwrap().0 & crate::mesh_tag::INTERIOR_FOG_BIT != 0
        };
        assert!(fog_of(&world, part), "gate on ⇒ the room's fog");

        // The camera moves out of the chain; the anchor does not move at all.
        world
            .get_mut::<WmoPortalInstance>(instance)
            .unwrap()
            .interior_fog[0] = false;
        world.run_system_once(classify_entity_interior).unwrap();
        assert!(!fog_of(&world, part), "gate off ⇒ back on the scene fog");
        assert_eq!(
            world.get::<InteriorLit>(part).unwrap().applied,
            Some(AppliedLaw::Matte),
            "and the LIGHT law is untouched — the unit is still standing in the same room"
        );

        // And back: the gate is a live read, not a latch.
        world
            .get_mut::<WmoPortalInstance>(instance)
            .unwrap()
            .interior_fog[0] = true;
        world.run_system_once(classify_entity_interior).unwrap();
        assert!(fog_of(&world, part), "the camera comes back into the chain");
    }

    /// The card is a world root, not the model's child, yet names the same anchor; also the
    /// classify-out arm, a WMO-display part with no interior variant.
    #[test]
    fn a_models_billboard_card_takes_the_same_law_as_its_mesh_parts() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = classifier_world();
        let generation = world.resource::<WmoResidency>().generation;
        let slot = world
            .resource_mut::<PropProbes>()
            .alloc_owned([Vec4::ZERO; 7])
            .unwrap();
        let anchor = world
            .spawn((
                GlobalTransform::default(),
                PropProbeSlot(slot),
                InteriorAnchor {
                    law: AppliedLaw::Bake(slot),
                    room: None,
                    fog: false,
                    last_pos: Vec3::ZERO,
                    generation,
                    kind_bake: true,
                },
            ))
            .id();

        let bake = Handle::default();
        let exterior = Handle::default();
        let lit_for = |anchor| {
            part_interior_lit(&exterior, Some(&exterior), Some(&bake), Vec3::ZERO, anchor)
                .expect("an entity M2 batch always builds an interior variant")
        };
        // The mesh batch rides the model's tree; the card is a world root. Nothing else differs.
        let mesh = world
            .spawn((
                lit_for(anchor),
                MeshMaterial3d::<WowModelMaterial>(Handle::default()),
                MeshTag(0),
            ))
            .id();
        world.entity_mut(anchor).add_child(mesh);
        let card = world
            .spawn((
                lit_for(anchor),
                MeshMaterial3d::<WowModelMaterial>(Handle::default()),
                MeshTag(crate::mesh_tag::alpha_bits(1.0)),
            ))
            .id();

        world.run_system_once(classify_entity_interior).unwrap();

        assert_eq!(
            world.get::<InteriorLit>(card).unwrap().applied,
            Some(AppliedLaw::Bake(slot)),
            "the world-root card takes its model's law, not the world's"
        );
        assert_eq!(
            world.get::<InteriorLit>(mesh).unwrap().applied,
            world.get::<InteriorLit>(card).unwrap().applied,
            "a model never splits across the two light laws at the billboard seam"
        );
        assert_eq!(
            world.get::<MeshTag>(card).unwrap().0,
            crate::mesh_tag::with_interior_probe(crate::mesh_tag::alpha_bits(1.0), slot),
            "the law composes into the tag — the probe slot lands, the card's alpha survives"
        );

        assert!(
            part_interior_lit(&exterior, None, None, Vec3::ZERO, anchor).is_none(),
            "a WMO-display part has no interior variant and classifies out entirely"
        );
    }

    /// Otherwise the part would ride the matte fallback until the anchor next moved.
    #[test]
    fn a_bake_part_joining_a_matte_resolved_anchor_forces_a_re_resolve() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = classifier_world();
        let generation = world.resource::<WmoResidency>().generation;
        let anchor = world
            .spawn((
                GlobalTransform::default(),
                InteriorAnchor {
                    law: AppliedLaw::Exterior,
                    room: None,
                    fog: false,
                    last_pos: Vec3::ZERO,
                    generation,
                    kind_bake: false,
                },
            ))
            .id();
        world.spawn((
            InteriorLit::new(
                InteriorKind::Bake {
                    material: Handle::default(),
                    center: Vec3::ZERO,
                },
                Handle::default(),
            ),
            ClassifiedBy(anchor),
            MeshMaterial3d::<WowModelMaterial>(Handle::default()),
            MeshTag(0),
        ));

        world.run_system_once(classify_entity_interior).unwrap();

        assert!(
            world.get::<InteriorAnchor>(anchor).is_none(),
            "the matte-resolved record is dropped so the next run re-rays"
        );
    }

    /// Otherwise an entity streaming in indoors fades in under exterior light and swaps laws when
    /// its 2 s ramp latches.
    #[test]
    fn an_anchor_whose_parts_are_all_fading_still_resolves() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = classifier_world();
        let anchor = world.spawn(GlobalTransform::default()).id();
        let part = world
            .spawn((
                InteriorLit::new(InteriorKind::Matte, Handle::default()),
                ClassifiedBy(anchor),
                MeshMaterial3d::<WowModelMaterial>(Handle::default()),
                MeshTag(crate::mesh_tag::alpha_bits(0.125)),
                crate::model_fade::RenderFade {
                    started: 0.0,
                    duration: 2.0,
                    from: 0.0,
                    to: 1.0,
                    curve: crate::model_fade::FadeCurve::Cubic,
                },
            ))
            .id();

        world.run_system_once(classify_entity_interior).unwrap();

        assert!(
            world.get::<InteriorAnchor>(anchor).is_some(),
            "the anchor resolves during the ramp, not two seconds after it"
        );
        assert!(
            world.get::<InteriorLit>(part).unwrap().applied.is_some(),
            "and the part records the law it resolved to"
        );
    }

    #[test]
    fn a_law_written_mid_ramp_keeps_the_alpha_and_takes_the_laws_blend_twin() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = classifier_world();
        let slot = world
            .resource_mut::<PropProbes>()
            .alloc_owned([Vec4::ZERO; 7])
            .unwrap();
        let generation = world.resource::<WmoResidency>().generation;
        let anchor = world
            .spawn((
                GlobalTransform::default(),
                PropProbeSlot(slot),
                InteriorAnchor {
                    law: AppliedLaw::Bake(slot),
                    room: None,
                    fog: false,
                    last_pos: Vec3::ZERO, // settled: no ray, the drain does the work
                    generation,
                    kind_bake: true,
                },
            ))
            .id();

        // The steady bake variant, and the probe-lit blend twin the ramp must feather on.
        let bake: Handle<WowModelMaterial> =
            bevy::asset::uuid_handle!("ba000000-0000-4000-8000-00000000ba1e");
        let bake_blend: Handle<WowModelMaterial> =
            bevy::asset::uuid_handle!("bb000000-0000-4000-8000-00000000b1e4");
        let exterior_blend: Handle<WowModelMaterial> =
            bevy::asset::uuid_handle!("e0000000-0000-4000-8000-00000000b1e4");
        let mid_ramp = crate::mesh_tag::alpha_bits(0.25);
        let part = world
            .spawn((
                InteriorLit::new(
                    InteriorKind::Bake {
                        material: bake.clone(),
                        center: Vec3::ZERO,
                    },
                    Handle::default(),
                ),
                ClassifiedBy(anchor),
                // Spawned on the exterior blend twin, as a streamed part is before it classifies.
                MeshMaterial3d::<WowModelMaterial>(exterior_blend.clone()),
                MeshTag(mid_ramp),
                crate::model_fade::FadeMaterials {
                    cutout: Handle::default(),
                    blend: exterior_blend.clone(),
                    bake_blend: Some(bake_blend.clone()),
                    zfill: None,
                },
                crate::model_fade::RenderFade {
                    started: 0.0,
                    duration: 2.0,
                    from: 0.0,
                    to: 1.0,
                    curve: crate::model_fade::FadeCurve::Cubic,
                },
            ))
            .id();

        world.run_system_once(classify_entity_interior).unwrap();

        let tag = world.get::<MeshTag>(part).unwrap().0;
        assert!(
            world.get::<InteriorLit>(part).unwrap().is_bake(),
            "the ramping part takes the room's bake law"
        );
        assert_eq!(
            tag,
            crate::mesh_tag::with_interior_probe(mid_ramp, slot),
            "the probe slot lands on top of the ramp's alpha, not over it"
        );
        assert_ne!(
            tag,
            crate::mesh_tag::probe_bits(slot),
            "the payload must not force a mid-ramp part opaque (a bare `probe_bits` write)"
        );
        let material = world
            .get::<MeshMaterial3d<WowModelMaterial>>(part)
            .unwrap()
            .0
            .clone();
        assert_eq!(
            material, bake_blend,
            "a ramping part takes the law's PROBE-LIT blend twin — not the exterior one it \
             spawned on (its light would read as full outdoor intensity), and not the steady bake \
             variant (the ramp would stop feathering)"
        );
        assert_ne!(material, exterior_blend);
        assert_ne!(material, bake);
    }

    #[test]
    fn a_fade_latch_enqueues_the_part_for_reauthoring() {
        let mut world = classifier_world();
        world.add_observer(enqueue_on_fade_latch);
        let part = world
            .spawn(InteriorLit::new(InteriorKind::Matte, Handle::default()))
            .id();
        world.resource_mut::<InteriorReauthor>().0.clear(); // drop the on_add entry
        world
            .entity_mut(part)
            .insert(crate::model_fade::RenderFade {
                started: 0.0,
                duration: 1.0,
                from: 0.0,
                to: 1.0,
                curve: crate::model_fade::FadeCurve::Cubic,
            });
        world
            .entity_mut(part)
            .remove::<crate::model_fade::RenderFade>();
        assert_eq!(
            world.resource::<InteriorReauthor>().0,
            vec![part],
            "the latch is the re-entry edge"
        );
    }

    /// A Stratholme portcullis spawns 15 cm under its corridor slab, so only its centre rays into
    /// the room. The centre is model-local, so a scaled placement carries it through the transform.
    #[test]
    fn a_gameobject_anchors_at_its_box_centre_and_a_unit_at_its_position() {
        use crate::wmo_portal::LightAttach;

        let at = GlobalTransform::from(
            Transform::from_translation(Vec3::new(10.0, 100.0, -5.0)).with_scale(Vec3::splat(2.0)),
        );
        let centre = BodyBakeCenter(Vec3::new(0.0, 3.5, 0.0));

        let (attach, anchor) = attach_anchor(true, Some(&centre), &at);
        assert_eq!(attach, LightAttach::Containment);
        assert_eq!(
            anchor,
            Vec3::new(10.0, 107.0, -5.0),
            "the containment anchor is the box centre through the placement — scale included"
        );

        let (attach, anchor) = attach_anchor(false, Some(&centre), &at);
        assert_eq!(attach, LightAttach::DownRay);
        assert_eq!(
            anchor,
            at.translation(),
            "a unit still rays from its position, box centre or not"
        );

        // No bounds yet (the model is still streaming): the origin.
        let (attach, anchor) = attach_anchor(true, None, &at);
        assert_eq!(attach, LightAttach::Containment);
        assert_eq!(anchor, at.translation());
    }

    #[test]
    fn slot_seat_survives_a_despawned_anchor_and_releases_the_orphan() {
        let mut world = World::new();
        world.init_resource::<PropProbes>();
        let coeffs = [Vec4::ZERO; 7];

        let alive = world.spawn_empty().id();
        let slot_a = world
            .resource_mut::<PropProbes>()
            .alloc_owned(coeffs)
            .unwrap();
        seat_probe_slot(&mut world.commands(), alive, slot_a);
        world.flush();
        assert_eq!(world.get::<PropProbeSlot>(alive).unwrap().0, slot_a);

        let doomed = world.spawn_empty().id();
        let slot_b = world
            .resource_mut::<PropProbes>()
            .alloc_owned(coeffs)
            .unwrap();
        let (live_before, _) = world.resource::<PropProbes>().occupancy();
        world.entity_mut(doomed).despawn();
        seat_probe_slot(&mut world.commands(), doomed, slot_b);
        world.flush();
        let (live_after, _) = world.resource::<PropProbes>().occupancy();
        assert_eq!(
            live_after,
            live_before - 1,
            "the orphan slot is released, not leaked"
        );
    }
}
