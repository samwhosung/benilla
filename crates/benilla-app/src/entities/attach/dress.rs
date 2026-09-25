//! Dressing a unit's model parts: the per-batch spawn shared by the first build and the in-place
//! re-dress ([`super::redress`]), so both dress a part by one law.

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

/// The batch a part was spawned from, its index into `DisplayModel::parts`, by which the re-dress
/// finds it again; stable for the visual's life, since only a display change (a teardown) rebuilds
/// `parts`. `card` is a billboard batch's world-root card, which does not despawn with its anchor.
#[derive(Component, Clone, Copy)]
pub(in crate::entities) struct DressedPart {
    pub(in crate::entities) index: u32,
    pub(in crate::entities) card: Option<Entity>,
}

/// The material variants one part draws through, after the character swap.
pub(super) struct PartMaterials<'a> {
    pub(super) steady: &'a Handle<WowModelMaterial>,
    pub(super) interior: Option<&'a Handle<WowModelMaterial>>,
    pub(super) fade_blend: Option<&'a Handle<WowModelMaterial>>,
    pub(super) bake: Option<&'a Handle<WowModelMaterial>>,
    pub(super) bake_blend: Option<&'a Handle<WowModelMaterial>>,
    /// The depth-prime twin, swapped per appearance too: its cutout samples the same texture.
    pub(super) zfill: Option<&'a Handle<WowModelMaterial>>,
}

/// `part`'s materials: its character slot's per-appearance set (body and extra skin at the
/// batch's own sidedness), else the ones the shared model was built with.
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

/// The material store and animated-material registries for a part that needs materials of its
/// own; taken at spawn, since the part's interior and fade records are built from the clones.
pub(super) struct OwnMats<'a> {
    pub(super) store: &'a mut Assets<WowModelMaterial>,
    pub(super) uv: &'a mut benilla_world::doodad_anim::UvAnimMaterials,
    pub(super) tint: &'a mut benilla_world::doodad_anim::TintAnimMaterials,
    pub(super) table: &'a mut benilla_world::mat_anim_table::MatAnimTable,
}

/// One part's own variant set, the owning form of [`PartMaterials`].
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

/// Give this part materials of its own, registered against its instance's clock, when its file
/// sequences bake different UV or tint loops: the registries key by material, so a shared one
/// cannot play two instances' slots. GameObjects only: no unit, player, corpse or held item in
/// the shipped data authors such a set (`benilla-extract entityuvscan`).
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
    // A shared variant may still be parked (`model_render::lazy`): realize it before reading.
    let mut clone = |h: &Handle<WowModelMaterial>| -> Option<Handle<WowModelMaterial>> {
        benilla_world::model_render::lazy::realize(own.store, h.id());
        let mut mat = own.store.get(h.id()).cloned()?;
        // A clone leaves the shared table: a carried slot would add the shared delta to its rows.
        mat.extension.anim_slots = Vec4::ZERO;
        Some(own.store.add(mat))
    };
    let owned = OwnedMaterials {
        steady: clone(src.steady)?,
        interior: src.interior.and_then(&mut clone),
        fade_blend: src.fade_blend.and_then(&mut clone),
        bake: src.bake.and_then(&mut clone),
        bake_blend: src.bake_blend.and_then(&mut clone),
        // Neither cloned nor registered, as on the doodad lane: it draws only while feathering.
        zfill: src.zfill.cloned(),
    };
    // Every variant the part can swap to takes a row, or it stops mid-scroll indoors or fading.
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

/// Put this part in `tick_anim_materials`' scan iff a row was registered for it: the shared UV
/// lane's in `entity_variants` (`UvLoops::animates`, which a rotate-only batch passes with no
/// `uv_anim`), or its own clone's, the only tint registration an entity gets and one that can
/// decline. A marker without a row trips the lane's check; a row without a marker freezes.
fn mark_mat_anim(
    child: &mut bevy::ecs::system::EntityCommands<'_>,
    part: &EntityPart,
    owned: &Option<OwnedMaterials>,
) {
    if part.uv_loops().animates() || owned.is_some() {
        child.insert(benilla_world::doodad_anim::AnimMatPart);
    }
}

/// Stamp a part or card with its kind's pick-population marker, which the mouseover pickers
/// filter by.
fn insert_pick_marker(child: &mut bevy::ecs::system::EntityCommands<'_>, kind: ModelKind) {
    match kind {
        ModelKind::GameObject => {
            child.insert(GoPickPart);
        }
        ModelKind::Creature => {
            child.insert(CreaturePickPart);
        }
        // Doodad and WMO parts are never dressed here.
        ModelKind::Doodad | ModelKind::Wmo => {}
    }
}

impl<'a> PartMaterials<'a> {
    /// This batch's [`FadeSet`], the handles the appear and despawn ramps feather through.
    pub(super) fn fade_set(&self) -> FadeSet<'a> {
        FadeSet {
            steady: self.steady,
            blend: self.fade_blend,
            bake_blend: self.bake_blend,
            zfill: self.zfill,
        }
    }
}

/// Everything a part's spawn reads from its unit, gathered once per visual.
pub(super) struct PartDress<'a> {
    /// The parent, the interior classifier's anchor and the clock every `MatAnim` follows.
    pub(super) unit: Entity,
    pub(super) kind: ModelKind,
    pub(super) char_mats: &'a CharSkinMaterials,
    /// Identity for the mouseover inspector, cloned onto every part and card.
    pub(super) object: &'a WorldObject,
    /// `0`: no palette slot, so parts draw the static bind-pose mesh.
    pub(super) inst_slot: u16,
    /// Whether the unit built a rig: its skinned parts leave the view cull to the root.
    pub(super) rigged: bool,
    /// Bone to anchor entity for billboard cards, resolved by the caller, since
    /// `RigPose::anchor_for` needs `&mut` on the pose.
    pub(super) anchors: std::collections::HashMap<u16, Entity>,
    /// The model-local interior fold point: one verdict per unit, so a body never splits across
    /// the two light laws.
    pub(super) bake_center: Vec3,
    /// The armed idle's authored CAaBox, the picker's volume for a skinned part.
    pub(super) idle_aabb: Option<Aabb>,
    /// `Time::elapsed_secs`, for a part joining a ramp already in progress.
    pub(super) now: f32,
    /// `Pending` for a fresh visual; a re-dress joins the unit's own appear-fade clock, so a new
    /// geoset neither pops opaque nor starts a second ramp.
    pub(super) fade: JoinedFade,
}

/// [`spawn_part`] for a merged group: the group part, its first member's index, and for more than
/// one member the list the re-dress reads back.
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

/// Spawn one mesh part or billboard card of a body under `dress.unit`; returns whether it armed
/// an appear-fade, which the attach path mirrors onto the root for later held items to join.
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
    // Resolved first: everything below dresses from it.
    let owned = own_per_sequence_materials(part, dress, own);
    let mats = match &owned {
        Some(o) => o.borrow(),
        None => part_materials(part, dress.char_mats),
    };
    let set = mats.fade_set();
    // A fresh part spawns on its blend twin at alpha ≈0 and a joiner at the ramp's current alpha,
    // so neither flashes for a frame before `apply_render_fade` ramps `α = t³`.
    let effective = PartFade::resolve(dress.fade, &set);
    let (init_mat, tag_alpha) = effective.seed(&set, dress.now);
    // A part with a palette slot and a skinned twin draws the twin (`WOW_RIG_SKIN`); any other
    // draws the static mesh.
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
        // The portrait booth's mirror: both mesh twins (the booth poses the skinned one at Stand,
        // as the reference's bake `0x524f60` does) and the steady material, never a fade variant.
        crate::portrait::PortraitPart {
            static_mesh: part.mesh.clone(),
            skinned_mesh: part.skinned_mesh.clone(),
            material: mats.steady.clone(),
        },
        dress.object.clone(),
        // The render meshes are `RENDER_WORLD`-only, so the ray pickers read this geometry.
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
    // Every part gets a tag, the instance slot and the live alpha: the slot is the instance's
    // identity (the tint reads it), not a skinning detail.
    child.insert(MeshTag(benilla_world::mesh_tag::spawn_tag(
        dress.inst_slot,
        tag_alpha,
    )));
    // The picker's skinned ray test links through `RigPart`, so only a skinned part takes it.
    if skinned {
        child.insert(benilla_world::rig_palette::RigPart(dress.unit));
    }
    if dress.rigged && part.skinned_mesh.is_some() {
        // The reference elects one sphere per object each frame, never one per batch
        // (`0x683340`), so the root's election owns the view cull (`exterior_cull`) and
        // `NoFrustumCulling` keeps Bevy's per-part test away. The `Aabb` then serves only the
        // picker: the idle's authored CAaBox, else the build-time bind box, which
        // `calculate_bounds` leaves alone on a `NoFrustumCulling` entity.
        let picker_aabb = dress.idle_aabb.or(part.aabb);
        if let Some(aabb) = picker_aabb {
            child.insert((aabb, NoFrustumCulling));
        }
    } else if let Some(aabb) = part.aabb {
        // A static part keeps Bevy's frustum cull on its build-time box: the `RENDER_WORLD`-only
        // mesh leaves `calculate_bounds` nothing to derive one from.
        child.insert(aabb);
    }
    // Indoor lighting, anchored at the unit root so every part shares its verdict: the footprint
    // MOCV bake (`0x69e4c0`, `0x6a7300`), with the matte ×1.0 as the bake's miss fallback.
    if let Some(lit) = part_interior_lit(
        mats.steady,
        mats.interior,
        mats.bake,
        dress.bake_center,
        dress.unit,
    ) {
        child.insert(lit);
    }
    // Animated material alpha (`0x707680`), authored per sequence: it samples the unit's own
    // `AnimationPlayer`, in phase with the pose.
    if let Some(anim) = &part.alpha_anim {
        child.insert(benilla_world::doodad_anim::MatAnim::following(
            anim.clone(),
            dress.unit,
        ));
    }
    mark_mat_anim(&mut child, part, &owned);
    effective.dress(&mut child, &set)
}

/// A billboard batch, whose transform belongs to the billboard system: a mirror anchor under the
/// unit for the portrait booths, and a world-root card following the batch's live joint, or the
/// anchor when the unit is unrigged.
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
                // A batch of the host's body, so a mount's glow card prunes with the mount
                // (`DressedLook::collect`).
                seat: crate::portrait::PortraitSeat::Body,
                kind: info.kind,
                // Not an attachment's sub-model: the reference's attach reset cannot reach it.
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
    // The reference has one instance alpha per model, so a card joins the unit's appear-fade like
    // any batch. It may need materials of its own like a mesh part: `G_FreezingTrap`'s animated
    // batch is its additive glow card, keyed in file slot 2 alone.
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
        // The picker's triangles, centred at the pivot like the render form.
        benilla_world::interact::PickMesh(part.geometry.clone()),
        card_follow,
    ));
    insert_pick_marker(&mut card, dress.kind);
    // The unit's instance slot, though the card is never skinned: a tinted unit tints its cards.
    card.insert(MeshTag(benilla_world::mesh_tag::spawn_tag(
        dress.inst_slot,
        tag_alpha,
    )));
    // The build-time bound: the `RENDER_WORLD`-only mesh leaves `calculate_bounds` nothing.
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
    // The batch's per-sequence alpha, off the same unit clock as the mesh parts.
    if let Some(anim) = &part.alpha_anim {
        card.insert(benilla_world::doodad_anim::MatAnim::following(
            anim.clone(),
            dress.unit,
        ));
    }
    mark_mat_anim(&mut card, part, &owned);
    let armed = effective.dress(&mut card, &set);
    let card = card.id();
    // The card does not despawn with the anchor, so the anchor names it for the re-dress to reap.
    commands.entity(anchor).insert(DressedPart {
        index: index as u32,
        card: Some(card),
    });
    armed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A standalone [`OwnMats`]; no test here dresses a GameObject, so its lane is never taken.
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

    /// One synthetic batch with its own distinguishable built material.
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

    /// A distinguishable `Uuid` handle, which needs no `Assets` store behind it.
    fn mat(n: u128) -> Handle<WowModelMaterial> {
        Handle::Uuid(
            bevy::asset::uuid::Uuid::from_u128(n + 1),
            std::marker::PhantomData,
        )
    }

    /// Six distinguishable handles.
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

    /// A robe skirt (geoset 1302) is authored two-sided while the closed body is not.
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

    /// `FadeMaterials` is a material record, not a fade record: the stream-out fade and the
    /// first-person feather read it off a part that spawned steady.
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

    /// The reference has one instance alpha per `CM2Model`, billboard batches included. Asserted
    /// on the card, not the anchor, which has no geometry.
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

    /// The marker set equals the `WorldObject`-kind set (the mesh part and the card, never the
    /// anchor), and the other kind's marker lands on nothing.
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

    /// A slot the look did not fill (a bald style, a non-fur race) keeps the model's built
    /// materials, like a batch with no slot, never another slot's set.
    #[test]
    fn an_unswapped_batch_keeps_the_models_own_materials() {
        let empty: CharSkinMaterials = (None, None, None, (None, None));
        let plain = part(None, false);
        assert_eq!(*part_materials(&plain, &empty).steady, plain.material);
        let mats: CharSkinMaterials = (Some((quint(1), quint(2))), None, None, (None, None));
        let bald = part(Some(CharSkinSlot::Hair), false);
        assert_eq!(*part_materials(&bald, &mats).steady, bald.material);
    }
}
