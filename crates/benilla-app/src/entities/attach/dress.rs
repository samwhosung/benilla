//! **Dressing** a unit's model parts — the per-batch spawn [`super::attach_entity_visuals`] runs
//! once per visual, factored out so the **re-dress** ([`super::redress`]) can run the identical law
//! over the parts a unit is already standing in.
//!
//! The reference never rebuilds a character to change what it is wearing: the composite atlas is
//! re-blitted into the component's own 256² target and the geosets are re-selected on the SAME
//! `CM2Model` (its per-instance visibility array `+0x98`, whose only writer is the character
//! compositor `0x477520`, through `0x7110d0`). Its attachments — held items, the helm, the
//! shoulders and everything hanging off them — are never touched by that path at all. This module
//! is the half of that law that concerns one part: which materials it draws through, and how it is
//! born.

use benilla_formats::CharSkinSlot;
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;
use benilla_world::billboard::BillboardCard;
use benilla_world::interact::{CreaturePickPart, GoPickPart, WorldObject};
use benilla_world::interior::part_interior_lit;
use benilla_world::model_fade::{FadeSet, JoinedFade, PartFade};
use benilla_world::model_render::{ModelKind, ModelPart};

use super::super::EntityPart;
use super::char_skin::CharSkinMaterials;

/// The display-model batch a spawned part child was built from — its index into
/// [`DisplayModel::parts`](super::super::DisplayModel), plus the world-root billboard card that
/// belongs to it when the batch is a camera-facing one (decision 0153: the card is a ROOT entity,
/// so it does not cascade with its anchor's despawn and must be named here).
///
/// This is the **re-dress edge** ([`super::redress`]): a gear change re-selects the geoset set and
/// re-composites the atlas, and each standing part is found again through this index — its geoset
/// id for "does it still show?", its char slot for "which atlas does it draw?". The index is stable
/// for as long as the visual lives, because the only thing that can rebuild `parts` is the display
/// itself changing, which is a teardown (`live_display::refresh_live_display`).
#[derive(Component, Clone, Copy)]
pub(in crate::entities) struct DressedPart {
    pub(in crate::entities) index: u32,
    pub(in crate::entities) card: Option<Entity>,
}

/// The material variants one part draws through, after the per-appearance character swap: the
/// steady exterior material, its interior-matte / interior-bake law twins, and the `Blend` twins the
/// appear/despawn ramps feather on.
///
/// One function answers it for both the spawn and the re-dress, which is the point: "which material
/// does this batch wear?" has exactly one implementation, so a gear change can never dress a body
/// differently from the way it was first built.
pub(super) struct PartMaterials<'a> {
    pub(super) steady: &'a Handle<WowModelMaterial>,
    pub(super) interior: Option<&'a Handle<WowModelMaterial>>,
    pub(super) fade_blend: Option<&'a Handle<WowModelMaterial>>,
    pub(super) bake: Option<&'a Handle<WowModelMaterial>>,
    pub(super) bake_blend: Option<&'a Handle<WowModelMaterial>>,
    /// The depth-prime twin (decision 0831) — carried per appearance like the rest, since its
    /// cutout discard samples the same swapped texture.
    pub(super) zfill: Option<&'a Handle<WowModelMaterial>>,
}

/// Select `part`'s materials: on a character-slot batch the per-appearance variants over the
/// composited body atlas / hair / cape / extra-skin textures (decisions 0041 / 0044 / 0045) — the
/// body and the extra skin at the batch's own sidedness, since a robe skirt is authored two-sided
/// and the closed body is not — and on every other batch the ones the shared model was built with.
pub(super) fn part_materials<'a>(
    part: &'a EntityPart,
    char_mats: &'a CharSkinMaterials,
) -> PartMaterials<'a> {
    let slot_mats = match part.char_slot {
        Some(CharSkinSlot::Body) => {
            char_mats
                .0
                .as_ref()
                .map(|(single, two)| if part.two_sided { two } else { single })
        }
        Some(CharSkinSlot::Hair) => char_mats.1.as_ref(),
        Some(CharSkinSlot::Object) => char_mats.2.as_ref(),
        Some(CharSkinSlot::SkinExtra) => {
            let (single, two) = &char_mats.3;
            if part.two_sided { two } else { single }.as_ref()
        }
        None => None,
    };
    match slot_mats {
        Some((ext, int, fade, bake, bake_blend, zfill)) => PartMaterials {
            steady: ext,
            interior: Some(int),
            fade_blend: Some(fade),
            bake: Some(bake),
            bake_blend: Some(bake_blend),
            zfill: zfill.as_ref(),
        },
        None => PartMaterials {
            steady: &part.material,
            interior: part.material_interior.as_ref(),
            fade_blend: part.fade_blend.as_ref(),
            bake: part.material_interior_bake.as_ref(),
            bake_blend: part.material_interior_bake_blend.as_ref(),
            zfill: part.zfill.as_ref(),
        },
    }
}

/// The material store and the two animated-material registries, for the one entity population
/// that needs a material of its **own** rather than the batch's (decision 2295).
///
/// Taken as one bundle, and taken by the dressing path itself rather than by a system that fixes
/// parts up afterwards, because the cloned handles have to be in hand *before* the part's
/// [`InteriorLit`](benilla_world::interior) and [`FadeMaterials`] are built from them — a later
/// pass would have to reach into both and rewrite what the classifier already wrote.
pub(super) struct OwnMats<'a> {
    pub(super) store: &'a mut Assets<WowModelMaterial>,
    pub(super) uv: &'a mut benilla_world::doodad_anim::UvAnimMaterials,
    pub(super) tint: &'a mut benilla_world::doodad_anim::TintAnimMaterials,
    pub(super) table: &'a mut benilla_world::mat_anim_table::MatAnimTable,
}

/// One part's **own** variant set — [`PartMaterials`]' owning twin, held by the spawn for as long
/// as it takes to dress the part.
struct OwnedMaterials {
    steady: Handle<WowModelMaterial>,
    interior: Option<Handle<WowModelMaterial>>,
    fade_blend: Option<Handle<WowModelMaterial>>,
    bake: Option<Handle<WowModelMaterial>>,
    bake_blend: Option<Handle<WowModelMaterial>>,
    zfill: Option<Handle<WowModelMaterial>>,
}

impl OwnedMaterials {
    fn borrow(&self) -> PartMaterials<'_> {
        PartMaterials {
            steady: &self.steady,
            interior: self.interior.as_ref(),
            fade_blend: self.fade_blend.as_ref(),
            bake: self.bake.as_ref(),
            bake_blend: self.bake_blend.as_ref(),
            zfill: self.zfill.as_ref(),
        }
    }
}

/// Give this part a material set of its **own**, registered against its own instance's anim host —
/// or `None`, which is every part but a measured thirteen batches (decision 2295).
///
/// **Who needs one.** A batch whose file-sequence slots bake *different* UV or tint loops cannot be
/// served by a material every instance of the model shares: the registries are keyed by material,
/// so one row would have to answer for placements that are playing different slots. That is
/// 1408's finding, and the entity side of it is six GameObject models — `BloodOfHeroes`'s five
/// bubble sheets (slot 0 holds them still, slot 1 sweeps them, at freq 16384/16383, so the pools
/// bubble independently), `Valentines_Blanket`, `G_GnomeTerminal`'s screen, and on the tint
/// channel `G_FreezingTrap`'s glow card, `OrgrimmarPentagram` and `ScholomanceCrystalBall01`.
/// **Four of the five tint batches bake nothing in slot 0**, so their shared material can only
/// ever seed white however faithfully it is ticked — the hunter's trap glows flat instead of
/// pulsing blue-white as it arms.
///
/// **Why it is gated on `GameObject`.** Not as a safety valve: the census is the reason. No unit,
/// no player body, no corpse and no held item in the shipped corpus authors a per-sequence set on
/// either channel (`benilla-extract entityuvscan`), so widening this past GameObjects would add a
/// clone-and-register branch to the hottest spawn path in the client for an empty population. If
/// a future model joins it, the gate is the line to move and this paragraph is why.
fn own_per_sequence_materials(
    part: &EntityPart,
    dress: &PartDress<'_>,
    own: &mut OwnMats<'_>,
) -> Option<OwnedMaterials> {
    let loops = part.uv_loops();
    let (uv_seq, tint_seq) = (loops.seqs.is_some(), part.rgb_seq.is_some());
    if dress.kind != ModelKind::GameObject || !(uv_seq || tint_seq) {
        return None;
    }
    let src = part_materials(part, dress.char_mats);
    // The shared variants may still be PARKED (`model_render::lazy` reserves a handle and holds
    // the value until something view-visible binds it), so each is realized before it is read —
    // the same order `spell_fx`'s clone takes, and the reason 2038's registrations all returned
    // `None` when they skipped it.
    let mut clone = |h: &Handle<WowModelMaterial>| -> Option<Handle<WowModelMaterial>> {
        benilla_world::model_render::lazy::realize(own.store, h.id());
        let mut mat = own.store.get(h.id()).cloned()?;
        // A clone leaves the shared table (decision 1381): its rows are its own, and a carried
        // slot would add the shared delta on top of them.
        mat.extension.anim_slots = Vec4::ZERO;
        Some(own.store.add(mat))
    };
    let owned = OwnedMaterials {
        steady: clone(src.steady)?,
        interior: src.interior.and_then(&mut clone),
        fade_blend: src.fade_blend.and_then(&mut clone),
        bake: src.bake.and_then(&mut clone),
        bake_blend: src.bake_blend.and_then(&mut clone),
        // The depth-prime twin is NOT cloned and NOT registered, exactly as on the placed-doodad
        // lane: it draws only while the part feathers, and its alpha test reading unscrolled UVs
        // for those frames is the behaviour the streamer has always had.
        zfill: src.zfill.cloned(),
    };
    // Every variant the part can swap to over its life — indoors, feathering, both — takes a row,
    // or a GameObject that walks under a roof would stop mid-scroll.
    for id in [
        Some(&owned.steady),
        owned.interior.as_ref(),
        owned.fade_blend.as_ref(),
        owned.bake.as_ref(),
        owned.bake_blend.as_ref(),
    ]
    .into_iter()
    .flatten()
    .map(Handle::id)
    {
        if uv_seq {
            benilla_world::doodad_anim::register_entity_uv(
                own.uv,
                own.table,
                own.store,
                id,
                &loops,
                Some(dress.unit),
            );
        }
        if let Some(seqs) = &part.rgb_seq {
            benilla_world::doodad_anim::register_tint(
                own.tint,
                own.table,
                own.store,
                id,
                benilla_world::doodad_anim::TintLoop::PerSeq {
                    seqs: seqs.clone(),
                    host: dress.unit,
                },
            );
        }
    }
    Some(owned)
}

/// Put this part in `tick_anim_materials`' draw scan — **iff something registered a row for it**.
///
/// The marker and the registration have to ask ONE question, because 2038's whole bug class is the
/// two disagreeing: a marked part with no registry entry trips the lane's own tripwire, and a
/// registered material that no marked part accounts for freezes silently, which is how every
/// waterfall in the game stood still for three days. So this is not a predicate over the part's
/// channels — it is a statement about what actually happened a few lines up.
///
/// Two registrations can have happened. The **shared** UV lane's, made inside
/// `entity_variants` when the batch's transform moves at all (`UvLoops::animates`, which is not
/// `uv_anim.is_some()`: a rotate-only batch answers `None` there). And this instance's **own**
/// clone, when it has one — which is the only way the TINT channel is ever registered on an
/// entity, and why `owned` has to be consulted rather than the part's `rgb_seq` re-tested: the
/// clone can decline (a full table, a dead handle), and a marker for a registration that did not
/// happen is the first half of the bug above.
fn mark_mat_anim(
    child: &mut bevy::ecs::system::EntityCommands<'_>,
    part: &EntityPart,
    owned: &Option<OwnedMaterials>,
) {
    if part.uv_loops().animates() || owned.is_some() {
        child.insert(benilla_world::doodad_anim::AnimMatPart);
    }
}

/// Stamp a pickable part (or card) with the pick population its kind belongs to: the mouseover
/// pickers filter by these markers instead of kind-comparing every `WorldObject` row per frame.
/// One site for both the mesh-part and the billboard-card spawn, so the two can't drift.
fn insert_pick_marker(child: &mut bevy::ecs::system::EntityCommands<'_>, kind: ModelKind) {
    match kind {
        ModelKind::GameObject => {
            child.insert(GoPickPart);
        }
        ModelKind::Creature => {
            child.insert(CreaturePickPart);
        }
        // Doodad/WMO parts (terrain_stream's spawns) are never dressed here and take no marker.
        ModelKind::Doodad | ModelKind::Wmo => {}
    }
}

impl<'a> PartMaterials<'a> {
    /// The three handles the appear/despawn ramps feather through — this batch's slice of
    /// [`FadeSet`], so the spawn and the fade can never disagree about which twin it wears.
    pub(super) fn fade_set(&self) -> FadeSet<'a> {
        FadeSet {
            steady: self.steady,
            blend: self.fade_blend,
            bake_blend: self.bake_blend,
            zfill: self.zfill,
        }
    }
}

/// Everything one part's spawn reads from its UNIT — gathered once per visual so the per-part body
/// isn't a twenty-argument call, and so the spawn path and the re-dress path provably feed it the
/// same things.
pub(super) struct PartDress<'a> {
    /// The unit whose model this is: the parent, the interior classifier's anchor, the animation
    /// clock every `MatAnim` follows.
    pub(super) unit: Entity,
    pub(super) kind: ModelKind,
    pub(super) char_mats: &'a CharSkinMaterials,
    /// Identity for the mouseover inspector, cloned onto every part and card.
    pub(super) object: &'a WorldObject,
    /// `0` = the palette table was full: parts fall back to the static bind-pose mesh.
    pub(super) inst_slot: u16,
    /// Whether the unit built a rig at all — the gate on a part drawing its skinned twin.
    pub(super) rigged: bool,
    /// Bone index → anchor entity, for a billboard batch's card to ride its live joint. Resolved
    /// by the CALLER through `RigPose::anchor_for` for exactly the card bones this dress will
    /// spawn (decision 1355) — owned, because the resolver needs `&mut` on the pose and this
    /// context is shared immutably across every part.
    pub(super) anchors: std::collections::HashMap<u16, Entity>,
    /// The model's interior fold reference (model-local) — one verdict per unit, so a body can
    /// never split across the two light laws.
    pub(super) bake_center: Vec3,
    /// The armed idle's authored CAaBox — the mouseover picker's volume for a skinned part.
    pub(super) idle_aabb: Option<Aabb>,
    /// `Time::elapsed_secs`, for a part joining a ramp already in progress.
    pub(super) now: f32,
    /// What this part should do about the unit's appear-fade. A fresh visual passes
    /// `Pending { since: now }` (decision 0032); a part spawned by a **re-dress** JOINS whatever the
    /// unit's own clock says ([`benilla_world::model_fade::join_unit_appear_fade`]), exactly as a
    /// late-resolving held item does — so a gear change during the login cascade neither pops the
    /// new geoset opaque over a still-feathering body nor restarts a second ramp beside it.
    pub(super) fade: JoinedFade,
}

/// Spawn one part of `dress.unit`'s model as a child of it, dressed by [`part_materials`]. Returns
/// whether it armed an appear-fade — the attach path mirrors that onto the unit root so a held item
/// resolving later can join the same ramp.
///
/// A **billboard** batch (a glow card / chain) can't spawn as an ordinary child: its mesh is centred
/// at the bone pivot and its transform belongs to the billboard system, so as a plain child it would
/// render at the model origin (the "glow on the ground" family, decision 0153). It spawns as a
/// lightweight mirror anchor under the unit plus a world-root card following the anchor's live joint.
/// [`spawn_part`] for a merged group (`attach::merge`): the synthetic group part, the first
/// member's index, and the member list the redress reads back. A singleton passes `None` and
/// is exactly [`spawn_part`].
pub(super) fn spawn_group(
    commands: &mut Commands,
    part: &EntityPart,
    group: &super::merge::BodyGroup,
    dress: &PartDress,
    own: &mut OwnMats<'_>,
) -> bool {
    let members =
        (group.members.len() > 1).then(|| super::merge::DressedGroup(group.members.clone().into()));
    spawn_part(commands, part, group.first() as usize, members, dress, own)
}

/// Spawn one mesh part (or billboard card) of a body under `dress.unit`; `group` is the merged
/// member list for a group of several batches (`None` for a batch of its own).
pub(super) fn spawn_part(
    commands: &mut Commands,
    part: &EntityPart,
    index: usize,
    group: Option<super::merge::DressedGroup>,
    dress: &PartDress,
    own: &mut OwnMats<'_>,
) -> bool {
    if let Some(info) = &part.billboard {
        return spawn_billboard_part(commands, part, index, info, dress, own);
    }
    // A material of this instance's OWN, for the measured thirteen batches whose slots disagree
    // (2295) — resolved first, because everything below dresses from whatever this returns.
    let owned = own_per_sequence_materials(part, dress, own);
    let mats = match &owned {
        Some(o) => o.borrow(),
        None => part_materials(part, dress.char_mats),
    };
    let set = mats.fade_set();
    // A freshly-streamed CGObject appear-fades in (decision 0032): spawn already on the blend twin
    // with a ≈0 `MeshTag`, so it doesn't flash opaque for a frame before `apply_render_fade` ramps
    // `α = t³`. A joiner computes the ramp's *current* alpha rather than 0, so it doesn't flash
    // invisible for a frame either.
    let effective = PartFade::resolve(dress.fade, &set);
    let (init_mat, tag_alpha) = effective.seed(&set, dress.now);
    // A skinned creature part draws its skinned-mesh twin (the WOW joint attributes → the
    // owned-palette `WOW_RIG_SKIN` shader path, decision 0720); everything else — and the
    // palette-full fallback (slot 0) — the static mesh. Keyed on the part having a twin, which the
    // slot alone no longer implies.
    let skinned = dress.inst_slot != 0 && part.skinned_mesh.is_some();
    let mesh = match &part.skinned_mesh {
        Some(sm) if skinned => sm.clone(),
        _ => part.mesh.clone(),
    };
    let mut child = commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(init_mat),
        Transform::default(),
        ChildOf(dress.unit),
        ModelPart {
            kind: dress.kind,
            blend: part.blend,
        },
        // The portrait booth mirrors this part ([`crate::portrait`]): both mesh twins — the booth
        // poses the skinned twin at Stand on its own throwaway skeleton (the ref bake `0x524f60`),
        // falling back to the static bind-pose twin for a boneless model — + the steady exterior
        // material (not the appear-fade/interior variant the child may wear now).
        crate::portrait::PortraitPart {
            static_mesh: part.mesh.clone(),
            skinned_mesh: part.skinned_mesh.clone(),
            material: mats.steady.clone(),
        },
        dress.object.clone(),
        // The picker's triangles (decision 0857): the render meshes are `RENDER_WORLD`-only, so
        // the ray pickers read the model's resident geometry instead.
        benilla_world::interact::PickMesh(part.geometry.clone()),
        DressedPart {
            index: index as u32,
            card: None,
        },
    ));
    if let Some(group) = group {
        child.insert(group);
    }
    insert_pick_marker(&mut child, dress.kind);
    // Every part gets a tag: the instance slot plus the live alpha field. Unconditional because the
    // slot is a per-instance identity every part needs (the tint) rather than a skinning detail —
    // and because the two conditional writers that used to seed it (the interior classifier, the
    // animated-alpha compose) each covered only their own subset, leaving a part in neither with no
    // tag at all.
    child.insert(MeshTag(benilla_world::mesh_tag::spawn_tag(
        dress.inst_slot,
        tag_alpha,
    )));
    // `RigPart` stays gated on actually being skinned by that rig — it is the CPU-side link for the
    // mouseover picker's skinned ray test (decision 0720), which has nothing to say about a static
    // part.
    if skinned {
        child.insert(benilla_world::rig_palette::RigPart(dress.unit));
    }
    if dress.rigged && part.skinned_mesh.is_some() {
        // A skinned entity part is never PART-culled: the reference tests ONE sphere per scene
        // object, never per batch, so the view cull belongs to the body ROOT's election
        // (`exterior_cull`, 1270) and `NoFrustumCulling` keeps Bevy's per-part test out of its
        // way. (0648 originally justified the marker as "the reference never view-culls entities
        // at all", off a ≈1e7 render-bounds reading, but those fields are a position cache
        // (`0x6717d0`), and units ARE frustum/horizon/room-elected per frame (`0x683340`).
        // Decision 1473 records the correction and owns the outdoor half of the election.)
        // Bevy's bind-pose `Aabb` is the wrong stand-in for picking (the duel flag plants itself
        // 9 yd below it) and is stomped anyway by
        // `calculate_bounds` on any `Mesh3d` change. So the `Aabb` beside the marker serves ONE
        // master: the mouseover picker (`target/hover.rs`) — the armed idle's authored CAaBox
        // when it has one, else the bind box, read from the part's build-time bound (decision
        // 0834: the static mesh is `RENDER_WORLD`-only, so its main-world data — which the old
        // `compute_aabb` fallback read — is gone after extract). Both `calculate_bounds` queries
        // skip `NoFrustumCulling` entities, so this box survives.
        let picker_aabb = dress.idle_aabb.or(part.aabb);
        if let Some(aabb) = picker_aabb {
            child.insert((aabb, NoFrustumCulling));
        }
    } else if let Some(aabb) = part.aabb {
        // A STATIC part (a boneless GameObject model, a WMO-display batch) keeps Bevy's ordinary
        // frustum cull — but its bind-pose box must be inserted here now (decision 0834):
        // `calculate_bounds` used to derive it from the mesh's main-world data, which the
        // `RENDER_WORLD`-only static form no longer has after extract.
        child.insert(aabb);
    }
    // M2 parts can light off a WMO room they stand in: a `MeshTag` + the classifier pick the law by
    // location (0354). Anchored at the unit root so every part shares the root's verdict, and the
    // indoor LAW is one for every entity M2 — the footprint-MOCV bake (`0x69e4c0`, `0x6a7300`),
    // with the matte ×1.0 as the bake's miss fallback.
    if let Some(lit) = part_interior_lit(
        mats.steady,
        mats.interior,
        mats.bake,
        dress.bake_center,
        dress.unit,
    ) {
        child.insert(lit);
    }
    // The batch's **animated material alpha** (the combine's runtime half, `0x707680`): a
    // creature's colour-alpha/transparency tracks are authored PER SEQUENCE, so which of its
    // batches draw is a function of what it is playing. Sampling follows the unit's own
    // `AnimationPlayer`, so the alpha stays in phase with the pose.
    if let Some(anim) = &part.alpha_anim {
        child.insert(benilla_world::doodad_anim::MatAnim::following(
            anim.clone(),
            dress.unit,
        ));
    }
    mark_mat_anim(&mut child, part, &owned);
    effective.dress(&mut child, &set)
}

/// The camera-facing half of [`spawn_part`]: a mirror anchor under the unit (so the portrait /
/// paper-doll booths can rebuild the batch — the visible card is a world ROOT and can never be
/// mirrored) plus the card itself, following the batch's live joint when the unit is rigged and the
/// anchor otherwise. Returns whether the card armed the unit's appear-fade, exactly as a mesh part
/// does: the batch is the model's, so it fades with the model (decision 0836).
fn spawn_billboard_part(
    commands: &mut Commands,
    part: &EntityPart,
    index: usize,
    info: &benilla_assets::BillboardInfo,
    dress: &PartDress,
    own: &mut OwnMats<'_>,
) -> bool {
    let anchor = commands
        .spawn((
            Transform::default(),
            Visibility::default(),
            ChildOf(dress.unit),
            crate::portrait::PortraitBillboard {
                mesh: part.mesh.clone(),
                material: part.material.clone(),
                bone: info.bone,
                // A rigged host's billboard bone already bakes the pivot into its booth joint (the
                // 0130 rig identity) — and the batch belongs to the host's body, so a mount's own
                // glow card prunes with the mount (`DressedLook::collect`).
                seat: crate::portrait::PortraitSeat::Body,
                kind: info.kind,
                // A batch of the HOST model, not a sub-model in an attachment node — the
                // reference's attach reset walks the attachment list and cannot reach it.
                attach: None,
            },
        ))
        .id();
    let (owner, at_joint) = match dress.anchors.get(&info.bone).copied() {
        Some(j) => (j, true),
        None => (anchor, false),
    };
    let card_follow = if at_joint {
        BillboardCard::following_joint(info, owner)
    } else {
        BillboardCard::following(info, owner)
    };
    // A card is a batch of the unit's own model, so it joins the unit's appear-fade like any other
    // batch of it — the reference has ONE instance alpha per model and every batch draws through it
    // (decision 0836). Splitting the batch into a world-root entity is benilla's parenting detail;
    // it is not a reason for the glow on a weapon's gems to blaze at full strength over a body that
    // has not appeared yet.
    // A card may need a material of its own for exactly the same reason a mesh part does — and
    // on this population it is the card that needs it: `World\Goober\G_FreezingTrap`'s animated
    // batch IS the additive glow card, whose blue-white pulse is keyed in file slot 2 alone.
    let owned = own_per_sequence_materials(part, dress, own);
    let mats = match &owned {
        Some(o) => o.borrow(),
        None => part_materials(part, dress.char_mats),
    };
    let set = mats.fade_set();
    let effective = PartFade::resolve(dress.fade, &set);
    let (init_mat, tag_alpha) = effective.seed(&set, dress.now);
    let mut card = commands.spawn((
        Mesh3d(part.mesh.clone()),
        MeshMaterial3d(init_mat),
        Transform::default(),
        ModelPart {
            kind: dress.kind,
            blend: part.blend,
        },
        dress.object.clone(),
        // The picker's triangles (decision 0857) — the caster centres a card at its pivot, the
        // same bake the render form draws with.
        benilla_world::interact::PickMesh(part.geometry.clone()),
        card_follow,
    ));
    insert_pick_marker(&mut card, dress.kind);
    // A card takes its MODEL's indoor law through the same constructor as the sibling meshes it was
    // split out of (decision 0778), and carries the unit's INSTANCE slot like every sibling part
    // even though it is never skinned by it — it is a batch of the unit's own model, so a tinted
    // unit tints its eye-glow and torch cards too (decision 0812). No char-slot variants are
    // consulted: a body/hair/cape/skin-extra batch is never a billboard batch.
    card.insert(MeshTag(benilla_world::mesh_tag::spawn_tag(
        dress.inst_slot,
        tag_alpha,
    )));
    // The card's build-time bound (decision 0834) — `calculate_bounds` can no longer derive one
    // from the `RENDER_WORLD`-only static form's data.
    if let Some(aabb) = part.aabb {
        card.insert(aabb);
    }
    if let Some(lit) = part_interior_lit(
        mats.steady,
        mats.interior,
        mats.bake,
        dress.bake_center,
        dress.unit,
    ) {
        card.insert(lit);
    }
    // A card shares its batch's per-sequence alpha loops (the billboard split copies them onto every
    // group), sampled off the same unit clock as the mesh parts.
    if let Some(anim) = &part.alpha_anim {
        card.insert(benilla_world::doodad_anim::MatAnim::following(
            anim.clone(),
            dress.unit,
        ));
    }
    // …and its batch's material animation, on the same rule as a mesh part (2295). A card is a
    // batch of the unit's own model; the split into a world-root entity is benilla's parenting
    // detail, never a reason for one batch of a model to animate and its sibling not to — and on
    // this population it is the card that carries it.
    mark_mat_anim(&mut card, part, &owned);
    let armed = effective.dress(&mut card, &set);
    let card = card.id();
    // The card follows the JOINT when the unit is rigged, so it does not cascade with the anchor's
    // despawn — the re-dress has to name it to reap it (see [`DressedPart::card`]).
    commands.entity(anchor).insert(DressedPart {
        index: index as u32,
        card: Some(card),
    });
    armed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A standalone [`OwnMats`] for the spawn tests — none of them dresses a GameObject, so the
    /// lane is never taken; it exists so the spawn's signature can stay non-optional, which is
    /// what stops a production caller passing `None` and silently losing the clone.
    #[derive(Default)]
    struct TestOwn {
        store: Assets<WowModelMaterial>,
        uv: benilla_world::doodad_anim::UvAnimMaterials,
        tint: benilla_world::doodad_anim::TintAnimMaterials,
        table: benilla_world::mat_anim_table::MatAnimTable,
    }

    impl TestOwn {
        fn lane(&mut self) -> OwnMats<'_> {
            OwnMats {
                store: &mut self.store,
                uv: &mut self.uv,
                tint: &mut self.tint,
                table: &mut self.table,
            }
        }
    }
    use benilla_world::model_fade::{FadeMaterials, PendingAppearFade};

    /// One synthetic batch at `char_slot`/`two_sided`, carrying its own (distinguishable) built
    /// material set so the fallback arm is visible in the assertions.
    fn part(char_slot: Option<CharSkinSlot>, two_sided: bool) -> EntityPart {
        EntityPart {
            mesh: Handle::default(),
            geometry: std::sync::Arc::new(benilla_formats::RenderSubmesh::default()),
            aabb: None,
            skinned_mesh: None,
            welded_billboard: false,
            material: mat(0),
            material_interior: None,
            material_interior_bake: None,
            material_interior_bake_blend: None,
            fade_blend: None,
            zfill: None,
            blend: benilla_formats::ModelBlend::Opaque,
            additive: false,
            two_sided,
            geoset_id: 0,
            char_slot,
            billboard: None,
            alpha_anim: None,
            rgb_anim: None,
            rgb_seq: None,
            uv_anim: None,
            uv_seq: None,
            uv_rot_seq: None,
            uv_scale_seq: None,
            ground_quad: None,
        }
    }

    /// A distinguishable material handle — a stable-id (`Uuid`) handle, which needs no `Assets`
    /// store behind it and compares by identity like any other.
    fn mat(n: u128) -> Handle<WowModelMaterial> {
        Handle::Uuid(
            bevy::asset::uuid::Uuid::from_u128(n + 1),
            std::marker::PhantomData,
        )
    }

    /// A set of six distinguishable handles, so which variant landed where is checkable.
    fn quint(seed: u128) -> super::super::char_skin::MatQuint {
        (
            mat(seed * 8),
            mat(seed * 8 + 1),
            mat(seed * 8 + 2),
            mat(seed * 8 + 3),
            mat(seed * 8 + 4),
            Some(mat(seed * 8 + 5)),
        )
    }

    /// The character swap picks a slot's quint — and the BODY (and extra skin) pick by the batch's
    /// own sidedness, because a robe skirt (geoset 1302) is authored two-sided while the closed body
    /// is not; flattening them to one variant culled the robe's inner faces (see-through from below).
    #[test]
    fn a_character_batch_takes_its_slots_variants_at_its_own_sidedness() {
        let single = quint(1);
        let two = quint(2);
        let hair = quint(3);
        let mats: CharSkinMaterials = (
            Some((single.clone(), two.clone())),
            Some(hair.clone()),
            None,
            (None, None),
        );

        let body_single = part(Some(CharSkinSlot::Body), false);
        assert_eq!(*part_materials(&body_single, &mats).steady, single.0);
        let body_two = part(Some(CharSkinSlot::Body), true);
        assert_eq!(*part_materials(&body_two, &mats).steady, two.0);

        let hair_part = part(Some(CharSkinSlot::Hair), false);
        let m = part_materials(&hair_part, &mats);
        assert_eq!(*m.steady, hair.0);
        assert_eq!(m.interior, Some(&hair.1));
        assert_eq!(m.fade_blend, Some(&hair.2));
        assert_eq!(m.bake, Some(&hair.3));
        assert_eq!(m.bake_blend, Some(&hair.4));
    }

    /// **`FadeMaterials` is a material RECORD, not a record of having armed a fade.** It used to be
    /// inserted only alongside the appear-fade arm, so a part spawned by anything that isn't a spawn
    /// — which, before decision 0835, meant a body rebuilt on *every gear change* — carried none at
    /// all. Two things read it and both silently stopped working for a player who had swapped so
    /// much as a shirt: the stream-out fade (`arm_despawn_descendants` gates on it) and, worse, the
    /// self-avatar first-person feather, whose query takes `&FadeMaterials` outright — so your own
    /// body stayed solid in your face. The item lane always did it this way; the body lane does now.
    #[test]
    fn a_steady_spawn_still_records_its_fade_materials() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>();
        let unit = app.world_mut().spawn(Transform::default()).id();
        let mut fadeable = part(None, false);
        fadeable.fade_blend = Some(mat(99));
        let anchors: std::collections::HashMap<u16, Entity> = std::collections::HashMap::new();
        let object = WorldObject {
            kind: ModelKind::Creature,
            label: String::new(),
            id: 0,
            detail: String::new(),
        };
        let empty: CharSkinMaterials = (None, None, None, (None, None));
        let dress = PartDress {
            unit,
            kind: ModelKind::Creature,
            char_mats: &empty,
            object: &object,
            inst_slot: 0,
            rigged: false,
            anchors,
            bake_center: Vec3::ZERO,
            idle_aabb: None,
            now: 0.0,
            fade: JoinedFade::Steady,
        };
        let mut own = TestOwn::default();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let armed = {
            let world = app.world();
            let mut commands = Commands::new(&mut queue, world);
            spawn_part(&mut commands, &fadeable, 0, None, &dress, &mut own.lane())
        };
        queue.apply(app.world_mut());
        assert!(!armed, "steady: no ramp armed");
        let mut q = app
            .world_mut()
            .query::<(&FadeMaterials, Has<PendingAppearFade>)>();
        let found: Vec<_> = q
            .iter(app.world())
            .map(|(fm, p)| (fm.blend.clone(), p))
            .collect();
        assert_eq!(found.len(), 1, "the record is kept even with no ramp");
        assert_eq!(found[0].0, mat(99), "…and names the part's own blend twin");
        assert!(!found[0].1, "…without arming an appear fade");
    }

    /// **A camera-facing batch fades with its model** (decision 0836): the split into a world-ROOT
    /// card is benilla's parenting detail (0153), not a law — the reference has one instance alpha
    /// per `CM2Model` and every batch draws through it, billboard batches included. The card used
    /// to open at a flat opaque with no arm and no record, which is a night-elf's eye glow and an
    /// undead's shoulder wisps burning at full strength over a body still at α 0.
    ///
    /// Asserted on the CARD, not the anchor: the anchor is a bare mirror carrier with no geometry.
    #[test]
    fn a_billboard_card_joins_the_units_appear_fade() {
        const SINCE: f32 = 5.0;
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>();
        let unit = app.world_mut().spawn(Transform::default()).id();
        let mut card = part(None, false);
        card.fade_blend = Some(mat(99));
        card.billboard = Some(benilla_assets::BillboardInfo {
            pivot: Vec3::ZERO,
            bone: 1,
            kind: benilla_formats::BillboardKind::Spherical,
            scale_anim: None,
            seq_translations: Vec::new(),
        });
        let anchors: std::collections::HashMap<u16, Entity> = std::collections::HashMap::new();
        let object = WorldObject {
            kind: ModelKind::Creature,
            label: String::new(),
            id: 0,
            detail: String::new(),
        };
        let empty: CharSkinMaterials = (None, None, None, (None, None));
        let dress = PartDress {
            unit,
            kind: ModelKind::Creature,
            char_mats: &empty,
            object: &object,
            inst_slot: 0,
            rigged: false,
            anchors,
            bake_center: Vec3::ZERO,
            idle_aabb: None,
            now: 0.0,
            fade: JoinedFade::Pending { since: SINCE },
        };
        let mut own = TestOwn::default();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let armed = {
            let world = app.world();
            let mut commands = Commands::new(&mut queue, world);
            spawn_part(&mut commands, &card, 0, None, &dress, &mut own.lane())
        };
        queue.apply(app.world_mut());
        assert!(
            armed,
            "a card arms the ramp like a mesh part — the attach path mirrors that onto the root",
        );
        let mut q = app.world_mut().query::<(
            &BillboardCard,
            &MeshTag,
            &MeshMaterial3d<WowModelMaterial>,
            &FadeMaterials,
            &PendingAppearFade,
        )>();
        let found: Vec<_> = q
            .iter(app.world())
            .map(|(_, t, m, fm, p)| (t.0, m.0.clone(), fm.blend.clone(), p.since))
            .collect();
        assert_eq!(found.len(), 1, "one billboard batch, one card");
        assert_eq!(found[0].3, SINCE, "joined the unit's own pending clock");
        assert_eq!(
            found[0].2,
            mat(99),
            "the record names the batch's blend twin"
        );
        assert_eq!(found[0].1, mat(99), "…and the card OPENS on it");
        assert!(
            benilla_world::mesh_tag::alpha_of(found[0].0) <= 1.0 / 63.0,
            "…at the encoder's ≈0 floor",
        );
    }

    /// The dress path stamps every pickable spawn — the mesh part and the billboard CARD (never
    /// the bare mirror anchor) — with exactly its kind's pick-population marker, so the mouseover
    /// pickers' marker-filtered sets equal the old kind-filtered ones by construction. Asserted
    /// differentially: the marker set must equal the `WorldObject`-kind set, and the other kind's
    /// marker must land on nothing.
    #[test]
    fn a_spawned_part_carries_its_kinds_pick_marker() {
        for kind in [ModelKind::Creature, ModelKind::GameObject] {
            let mut app = App::new();
            app.add_plugins((MinimalPlugins, AssetPlugin::default()))
                .init_asset::<Mesh>();
            let unit = app.world_mut().spawn(Transform::default()).id();
            let mut billboard = part(None, false);
            billboard.billboard = Some(benilla_assets::BillboardInfo {
                pivot: Vec3::ZERO,
                bone: 1,
                kind: benilla_formats::BillboardKind::Spherical,
                scale_anim: None,
                seq_translations: Vec::new(),
            });
            let object = WorldObject {
                kind,
                label: String::new(),
                id: 0,
                detail: String::new(),
            };
            let empty: CharSkinMaterials = (None, None, None, (None, None));
            let dress = PartDress {
                unit,
                kind,
                char_mats: &empty,
                object: &object,
                inst_slot: 0,
                rigged: false,
                anchors: std::collections::HashMap::new(),
                bake_center: Vec3::ZERO,
                idle_aabb: None,
                now: 0.0,
                fade: JoinedFade::Steady,
            };
            let mut own = TestOwn::default();
            let mut queue = bevy::ecs::world::CommandQueue::default();
            {
                let world = app.world();
                let mut commands = Commands::new(&mut queue, world);
                spawn_part(
                    &mut commands,
                    &part(None, false),
                    0,
                    None,
                    &dress,
                    &mut own.lane(),
                );
                spawn_part(&mut commands, &billboard, 1, None, &dress, &mut own.lane());
            }
            queue.apply(app.world_mut());
            let world = app.world_mut();
            let by_kind: std::collections::HashSet<Entity> = world
                .query::<(Entity, &WorldObject)>()
                .iter(world)
                .filter(|(_, o)| o.kind == kind)
                .map(|(e, _)| e)
                .collect();
            let (creature, go) = (
                world
                    .query_filtered::<Entity, With<CreaturePickPart>>()
                    .iter(world)
                    .collect::<std::collections::HashSet<_>>(),
                world
                    .query_filtered::<Entity, With<GoPickPart>>()
                    .iter(world)
                    .collect::<std::collections::HashSet<_>>(),
            );
            let (marked, other) = match kind {
                ModelKind::Creature => (creature, go),
                _ => (go, creature),
            };
            assert_eq!(
                by_kind.len(),
                2,
                "the mesh part and the card, not the anchor"
            );
            assert_eq!(marked, by_kind, "marker set == kind set ({kind:?})");
            assert!(other.is_empty(), "never the other kind's marker ({kind:?})");
        }
    }

    /// A batch with no character slot — and a slot whose row the appearance didn't resolve (a bald
    /// style has no hair texture, only tauren author an extra skin) — keeps the shared model's own
    /// built materials. That is the arm a creature, a GameObject and an unresolvable look all take.
    #[test]
    fn an_unswapped_batch_keeps_the_models_own_materials() {
        let empty: CharSkinMaterials = (None, None, None, (None, None));
        let plain = part(None, false);
        assert_eq!(*part_materials(&plain, &empty).steady, plain.material);
        // A slot the look didn't fill falls to the same place, not to another slot's quint.
        let mats: CharSkinMaterials = (Some((quint(1), quint(2))), None, None, (None, None));
        let bald = part(Some(CharSkinSlot::Hair), false);
        assert_eq!(*part_materials(&bald, &mats).steady, bald.material);
    }
}
