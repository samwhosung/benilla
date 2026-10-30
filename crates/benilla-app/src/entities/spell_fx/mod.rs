//! Spell-visual effect models: the attach-point `.mdx` a kit hangs on a unit, and the
//! lootable-corpse sparkle, which the client hangs on the same `Effect_C` node type.
//!
//! Each instance root is parented once under the unit's attach joint and rides the bone through
//! the transform tree, as the reference's `0x620be0` parents the effect model to the host once and
//! its transform refresh `0x714000` carries it. Every instance takes `CMissile`'s attach cascade
//! (`ATTACH_FALLBACKS`); the reference's kit effect has none, since `AddEffect 0x61fdd0` tests no
//! attachment, so on a model lacking the tag `0x620be0` draws nothing and never retries.
//!
//! A persistent instance lives until its spell's [`SpellKitFx::Reap`] (`0x614150`); a
//! self-terminating one despawns after one pass of its model's sequence 0, a span clock standing in
//! for the client's completion callback. A rebuild of the unit's visual ends both first
//! ([`RebuildFx`]).

mod lifecycle;

use std::collections::HashMap;

use benilla_assets::bone_target_id;
use bevy::animation::AnimatedBy;
use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use crate::creature_anim::spell_visual::FxSlot;
use crate::creature_anim::{scan_events, AnimSoundEvent, FxClass, FxStage, SpellKitFx};
use benilla_assets::m2_url;
use benilla_assets::materials::WowModelMaterial;
use benilla_world::model_render::{ModelKind, ModelPart};
use benilla_world::particles;
use benilla_world::vis_chain::VisChainOnly;

use super::{BoneAttach, DisplayModel, EntityPart, ModelHandle};
pub(super) use lifecycle::advance_fx_anim;
use lifecycle::{decay_span, FxAnimLife, FxDecay};

/// `CMissile`'s attach cascade (`0x61ceb0`) for a model lacking the requested point: `0xf`, then
/// `0x13`, then the unit root.
const ATTACH_FALLBACKS: [u16; 2] = [0xf, 0x13];

/// The self-termination span of a model with no sequence table; [`super::dest_fx`] shares it.
pub(crate) const FALLBACK_SPAN: f32 = 1.0;

/// How long a self-terminating instance may wait for a model that never loads.
const PENDING_TIMEOUT: f32 = 10.0;

/// The effect-model cache by path; `super::update_display_models` builds each entry's parts.
#[derive(Resource, Default)]
pub(crate) struct SpellFx {
    pub(crate) models: HashMap<String, DisplayModel>,
}

/// Get-or-insert this path's cache entry and start its load: the only way to reach
/// [`SpellFx::models`], since a map change clears it and a standing aura never re-sends `Begin`.
pub(super) fn ensure_model(fx: &mut SpellFx, asset_server: &AssetServer, path: &str) {
    fx.models
        .entry(path.to_string())
        .or_insert_with(|| DisplayModel {
            handle: ModelHandle::M2(asset_server.load(m2_url(path))),
            ..super::empty_shell()
        });
}

/// Per-instance tint clones, material id → `(RGB loop, attach time)`: one cast is one phase, so an
/// animated M2Color cannot share the doodad lane's global clock. An entry drops with its material.
#[derive(Resource, Default)]
pub(crate) struct FxTintAnims(
    HashMap<
        bevy::asset::AssetId<WowModelMaterial>,
        (std::sync::Arc<benilla_formats::RgbAnim>, f32),
    >,
);

/// Re-sample every live clone's tint on its instance clock. Not capture-gated: `fxview` ages an
/// effect inside a capture, and no golden scenario spawns one.
pub(crate) fn tick_fx_tint(
    time: Res<Time>,
    mut reg: ResMut<FxTintAnims>,
    mut materials: ResMut<Assets<WowModelMaterial>>,
) {
    if reg.0.is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    reg.0.retain(|id, (anim, origin)| {
        let Some(mat) = materials.get_mut(*id) else {
            return false; // the instance despawned, and its clone with it
        };
        let rgb = anim.sample(now - *origin);
        mat.extension.tint = Vec4::new(rgb[0], rgb[1], rgb[2], 1.0);
        true
    });
}

/// The material store and the per-instance animation registries a part's clone joins.
pub(crate) struct FxMaterials<'a> {
    pub(crate) store: &'a mut Assets<WowModelMaterial>,
    pub(crate) tint: &'a mut FxTintAnims,
    pub(crate) uv: &'a mut benilla_world::doodad_anim::UvAnimMaterials,
    pub(crate) table: &'a mut benilla_world::mat_anim_table::MatAnimTable,
}

/// One part's material for a new instance: the shared handle, or a per-instance clone when its RGB
/// or texture transform animates. The UV lane reads `host`'s player.
fn fx_part_material(
    part: &EntityPart,
    now: f32,
    host: Entity,
    mats: &mut FxMaterials,
) -> Handle<WowModelMaterial> {
    let loops = part.uv_loops();
    if part.rgb_anim.is_none() && !loops.any() {
        return part.material.clone();
    }
    // The shared material may still be parked (`model_render::lazy`).
    benilla_world::model_render::lazy::realize(mats.store, part.material.id());
    let Some(mut mat) = mats.store.get(part.material.id()).cloned() else {
        return part.material.clone(); // not built yet, though the parts were checked ready
    };
    if let Some(anim) = &part.rgb_anim {
        let t0 = anim.sample(0.0);
        mat.extension.tint = Vec4::new(t0[0], t0[1], t0[2], 1.0);
    }
    // Off the shared mat-anim table, or its delta would add to the clone's own rows.
    mat.extension.anim_slots = Vec4::ZERO;
    if loops.any() {
        // Seed at t = 0: the delta table measures from `sun_scale.zw` at registration.
        let t0 = loops.open_offset();
        mat.extension.sun_scale.z = t0[0];
        mat.extension.sun_scale.w = t0[1];
    }
    let handle = mats.store.add(mat);
    if let Some(anim) = &part.rgb_anim {
        mats.tint.0.insert(handle.id(), (anim.clone(), now));
    }
    if loops.any() {
        benilla_world::doodad_anim::register_fx_uv(
            mats.uv,
            mats.table,
            mats.store,
            handle.id(),
            loops,
            host,
            now,
        );
    }
    handle
}

/// Where an effect-model instance sits in the client's model graph.
#[derive(Clone, Copy, Default)]
pub(crate) struct EffectHost {
    /// The model this instance is chained to ([`benilla_world::model_fade::ParentModel`]), if any.
    pub parent: Option<Entity>,
}

/// Attach one effect model's visuals to `root`, for every effect consumer. Part meshes go unculled,
/// since a bind-pose bound is wrong for a billboarded subtree; a `ground_anchor` instance's flat
/// ground quads become projected decals ([`crate::ground_fx`]). Returns `false` while the model's
/// parts still build.
pub(crate) fn attach_effect_visuals(
    commands: &mut Commands,
    root: Entity,
    dm: &DisplayModel,
    now: f32,
    ground_anchor: bool,
    host: EffectHost,
    stage: Option<FxStage>,
    mats: &mut FxMaterials,
    ibps: &Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>,
    palettes: &mut benilla_world::rig_palette::RigPalettes,
    preferred_anim: Option<u16>,
) -> bool {
    let Some(parts) = dm.parts.as_ref() else {
        return false; // still loading
    };
    // Chained onto its host model, its alpha composes through the parent's, as `0x714000` does.
    if let Some(parent) = host.parent {
        commands
            .entity(root)
            .insert(benilla_world::model_fade::ParentModel(parent));
    }
    // Freed with the host model (the reference's destructor); a free-standing missile drains.
    let on_owner_loss = if host.parent.is_some() {
        benilla_world::particles::OwnerLoss::Free
    } else {
        benilla_world::particles::OwnerLoss::Drain
    };
    let is_ground_decal = |part: &EntityPart| ground_anchor && part.ground_quad.is_some();
    let part_materials: Vec<Handle<WowModelMaterial>> = parts
        .iter()
        .map(|p| {
            if is_ground_decal(p) {
                p.material.clone()
            } else {
                fx_part_material(p, now, root, mats)
            }
        })
        .collect();
    let (joints, armed) = arm_effect_rig(commands, root, dm, preferred_anim, stage);
    let rig_slot = match (&dm.inverse_bindposes, joints.is_empty()) {
        (Some(ibp), false) => {
            benilla_world::rig_palette::RigSkin::allocate(palettes, joints.clone(), ibp.clone())
                .map_or(0, |rig| {
                    let slot = rig.slot;
                    commands.entity(root).insert(rig);
                    slot
                })
        }
        _ => 0,
    };
    let rigged = rig_slot != 0;
    // The opening clip's file sequence slot keys each batch's per-sequence alpha loops.
    let played = dm
        .animations
        .as_ref()
        .and_then(|a| a.preferred_clip(preferred_anim));
    let played_seq = played.map(|c| c.seq_index);
    // A `CEffect`'s riders follow its player's live sequence; other lanes keep the opening one.
    let seq_host = (armed && stage.is_some()).then_some(root);
    commands.entity(root).with_children(|children| {
        for (part, material) in parts.iter().zip(&part_materials) {
            if part.billboard.is_some() {
                continue; // spawned as a following card below
            }
            if is_ground_decal(part) {
                continue; // spawned as a projected surface decal below
            }
            let mesh = match (rigged, &part.skinned_mesh) {
                (true, Some(sm)) => sm.clone(),
                _ => part.mesh.clone(),
            };
            let mut child = children.spawn((
                Mesh3d(mesh),
                MeshMaterial3d(material.clone()),
                Transform::default(),
                bevy::camera::visibility::NoFrustumCulling,
                ModelPart {
                    kind: ModelKind::Creature,
                    blend: part.blend,
                },
                // The picker's triangles; the render meshes are `RENDER_WORLD`-only.
                benilla_world::interact::PickMesh(part.geometry.clone()),
            ));
            if let (true, Some(_)) = (rigged, &part.skinned_mesh) {
                child.insert((
                    benilla_world::rig_palette::RigPart(root),
                    bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(rig_slot, 1.0)),
                ));
            }
            // The alpha loops on this instance's clock; the sampler is the tag's only writer.
            if let Some(anim) = &part.alpha_anim {
                let mat_anim =
                    benilla_world::doodad_anim::MatAnim::driving_tag(anim.clone(), now, played_seq)
                        .following_host(seq_host);
                let tag_slot = if rigged && part.skinned_mesh.is_some() {
                    rig_slot
                } else {
                    0
                };
                child.insert((
                    bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(
                        tag_slot,
                        mat_anim.current,
                    )),
                    mat_anim,
                ));
            }
        }
    });
    for (part, material) in parts.iter().zip(&part_materials) {
        let Some(info) = &part.billboard else {
            continue;
        };
        let card = match joints.get(info.bone as usize) {
            Some(&j) => benilla_world::billboard::BillboardCard::following_joint(info, j),
            None => benilla_world::billboard::BillboardCard::following(info, root),
        };
        let mut spawned = commands.spawn((
            Mesh3d(part.mesh.clone()),
            MeshMaterial3d(material.clone()),
            Transform::default(),
            ModelPart {
                kind: ModelKind::Creature,
                blend: part.blend,
            },
            // The picker's triangles, pivot-centred by the caster like the bake.
            benilla_world::interact::PickMesh(part.geometry.clone()),
            card,
        ));
        // The card's build-time bound: `calculate_bounds` cannot read the `RENDER_WORLD`-only mesh.
        if let Some(aabb) = part.aabb {
            spawned.insert(aabb);
        }
        // A card shares its batch's material-alpha loops (the billboard split copies them).
        if let Some(anim) = &part.alpha_anim {
            let mat_anim =
                benilla_world::doodad_anim::MatAnim::driving_tag(anim.clone(), now, played_seq)
                    .following_host(seq_host);
            spawned.insert((
                bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(0, mat_anim.current)),
                mat_anim,
            ));
        }
    }
    // Ground quads as projected decals on their joints: texture, blend, fog policy (`0x70baf0`) and
    // RGB loop ride the record. No UV scroll: no effect model with a texture transform has a ground
    // quad (`benilla-extract fxuvscan`).
    let binds = dm.inverse_bindposes.as_ref().and_then(|h| ibps.get(h));
    for part in parts.iter() {
        let Some(quad) = part.ground_quad.filter(|_| is_ground_decal(part)) else {
            continue;
        };
        let (joint, ibp) = match joints.get(quad.bone as usize) {
            Some(&j) => (
                j,
                binds
                    .and_then(|b| b.get(quad.bone as usize))
                    .copied()
                    .unwrap_or(Mat4::IDENTITY),
            ),
            None => (root, Mat4::IDENTITY), // boneless model: the quad rides the instance root
        };
        // The material's packed fog bits, raw; Scene (7) with no material, the packer's default.
        benilla_world::model_render::lazy::realize(mats.store, part.material.id());
        let (texture, fog_bits) = match mats.store.get(part.material.id()) {
            Some(mat) => (
                mat.base.base_color_texture.clone(),
                (mat.extension.clutter_fade.z as u32 >> 4) & 7,
            ),
            None => (None, u32::from(benilla_formats::FogPolicy::Scene as u8)),
        };
        let Some(texture) = texture else {
            continue; // no texture, nothing to draw
        };
        let decal = benilla_world::ground_fx::spawn_ground_fx_decal(
            commands,
            texture,
            part.blend,
            part.additive,
            fog_bits,
            part.rgb_anim.as_ref().map(|a| (a.clone(), now)),
            &quad,
            joint,
            ibp,
        );
        if let Some(anim) = &part.alpha_anim {
            let mat_anim =
                benilla_world::doodad_anim::MatAnim::driving_tag(anim.clone(), now, played_seq)
                    .following_host(seq_host);
            commands.entity(decal).insert(mat_anim);
        }
    }
    for em in &dm.emitters {
        let owner = joints
            .get(em.def.bone as usize)
            .map_or((root, [0.0; 3]), |&j| (j, em.bone_pivot));
        particles::spawn_emitter(
            commands,
            em,
            Transform::IDENTITY,
            particles::EmitterFrames {
                owner: Some(owner),
                // The cloud sorts at the instance root; the bone only places births.
                anchor: Some(root),
                on_owner_loss,
                // The particles' model is this instance, whose chain carries the host's fade.
                alpha: Some(root),
                // Every `Spells\` emitter sets the unlit bit; a lit one would take the scene light.
                light_node: None,
            },
            // Rate and enable windows follow the current sequence, as the animate kernel `0x714260`
            // samples it; global sequences run on the instance's own age.
            match seq_host {
                Some(h) => particles::EmitClock::Host(h),
                None => particles::EmitClock::Effect(played_seq),
            },
        );
    }
    for rb in &dm.ribbons {
        let (owner, use_pivot) = joints
            .get(rb.def.bone as usize)
            .map_or((root, false), |&j| (j, true));
        benilla_world::ribbons::spawn_ribbon(
            commands,
            rb,
            owner,
            use_pivot,
            // Unscaled, like the emitters' identity placement, so both land on one draw-order rung.
            1.0,
            // The visibility gate reads the root's live sequence, so it follows the lifecycle.
            benilla_world::ribbons::RibbonSeq::Host(root),
            // This instance's model alpha, chained to its host.
            Some(root),
            // No fade sphere: an effect instance is not a placed model.
            None,
        );
    }
    true
}

/// Arm an effect instance's rig under `root`, returning the joints and whether a player was armed.
/// The clip plays even with constant keys, unlike the doodad gate (`ModelAnimations::first_seq`),
/// because emitters read the posed joints.
pub(super) fn arm_effect_rig(
    commands: &mut Commands,
    root: Entity,
    dm: &DisplayModel,
    preferred_anim: Option<u16>,
    stage: Option<FxStage>,
) -> (Vec<Entity>, bool) {
    if dm.skeleton.joints.is_empty() {
        return (Vec::new(), false);
    }
    let joints = benilla_world::rig_palette::spawn_joints(commands, root, root, &dm.skeleton);
    // Billboard bones face the camera at the palette level, their children inheriting.
    if let Some(bb) = benilla_world::billboard::BillboardJointRig::new(&dm.skeleton, &joints, root)
    {
        commands.entity(root).insert(bb);
    }
    let mut armed = false;
    if let Some(anims) = dm.animations.as_ref() {
        // The caller's sequence (a thrown weapon's InFlight), else the bootstrap's `Stand`.
        if let Some(clip) = anims.preferred_clip(preferred_anim) {
            let mut player = AnimationPlayer::default();
            // The stage owns the repeat policy (`0x60ed00` repeats a clamping precast).
            let life = FxAnimLife::arm(&mut player, clip, stage.unwrap_or(FxStage::OneShot));
            commands
                .entity(root)
                .insert((player, AnimationGraphHandle(anims.graph.clone())));
            // The lifecycle watcher, on a `CEffect` only.
            if stage.is_some() {
                commands.entity(root).insert((life, anims.clone()));
            }
            for (i, &j) in joints.iter().enumerate() {
                commands
                    .entity(j)
                    .insert((bone_target_id(i as u16), AnimatedBy(root)));
            }
            armed = true;
        }
        if let Some(drive) =
            benilla_world::rig_anim::GlobalSeqDrive::new(&anims.global_bones, &joints)
        {
            // Fresh per play: the reference makes a model per cast, so global sequences open at 0.
            commands.entity(root).insert(drive);
        }
    }
    (joints, armed)
}

/// One live (or pending) effect-model instance on a unit.
struct FxInstance {
    /// The reap key (the client's `CEffect+0x18`); the lootable-corpse sparkle uses `u32::MAX`.
    spell_id: u32,
    /// Reaped by spell id (precast, channel) rather than self-terminating (cast release).
    persistent: bool,
    /// Which reap can end a persistent instance, the client's discriminator beside the spell id.
    class: FxClass,
    /// The lifecycle, apart from `class`: one class covers a stage-4 precast and a stage-2 channel.
    stage: FxStage,
    /// The M2 attachment id to hang from ([`benilla_formats::KIT_SLOT_TAGS`]), or
    /// [`benilla_formats::WORLD_EFFECT_TAG`] for the field-12 world-plant slot.
    tag: u16,
    /// The `SpellVisualEffectName` id; with `tag`, the same-slot replace key (`0x6208e0`).
    effect: u32,
    path: String,
    root: Option<Entity>,
    /// The self-termination deadline on `time.elapsed_secs()`, or a reaped instance's `Decay` end.
    expires: Option<f32>,
    /// Reaped and playing its `Decay` out (`0x6141f0`). A reap or replace cannot reach it, as the
    /// reference has moved it to its pending-destroy list.
    decaying: bool,
}

/// A unit's effect instances (the client's per-unit `+0xb4` effect list).
#[derive(Component, Default)]
pub(super) struct FxAttached {
    instances: Vec<FxInstance>,
}

/// A world-planted kit instance root (kit field 12): a free entity planted once at its owner
/// (`0x620a90`), and re-planted on displacement for a root-aura spell (`0x620580`).
#[derive(Component)]
pub(super) struct WorldPlantFx {
    owner: Entity,
    /// Re-plant on owner displacement (the client's flag `0x4000`): root-aura persistents only.
    follow: bool,
}

/// The client's re-plant gate, distance² > 1e-3 in world units (`0x620580`).
const REPLANT_EPS_SQ: f32 = 1e-3;

/// The plant transform (`0x620a90`): the owner's position, yaw only and scale.
fn world_plant_transform(owner: &GlobalTransform) -> Transform {
    let (scale, rotation, translation) = owner.to_scale_rotation_translation();
    let (yaw, _, _) = rotation.to_euler(EulerRot::YXZ);
    Transform {
        translation,
        rotation: Quat::from_rotation_y(yaw),
        scale,
    }
}

/// Whether an `EffectApplyAuraName` slot is `SPELL_AURA_MOD_ROOT` (26): the client's
/// `spellRec+0x16c[0..2] == 0x1a` scan that arms the re-plant flag `0x4000` (`0x60f081`).
fn spell_has_root_aura(spells: Option<&crate::ui_action::Spells>, spell_id: u32) -> bool {
    const SPELL_AURA_MOD_ROOT: u32 = 26;
    spells
        .and_then(|s| s.catalog.get(spell_id))
        .is_some_and(|d| d.effect_apply_aura.contains(&SPELL_AURA_MOD_ROOT))
}

/// Despawn a plant whose owner is gone, and re-plant a `follow` one whose owner moved past
/// [`REPLANT_EPS_SQ`] (`0x620580`, per frame).
pub(super) fn tend_world_plants(
    mut commands: Commands,
    mut plants: Query<(Entity, &WorldPlantFx, &mut Transform)>,
    owners: Query<&GlobalTransform>,
) {
    for (root, plant, mut t) in &mut plants {
        let Ok(owner) = owners.get(plant.owner) else {
            commands.entity(root).despawn();
            continue;
        };
        if plant.follow && owner.translation().distance_squared(t.translation) > REPLANT_EPS_SQ {
            *t = world_plant_transform(owner);
        }
    }
}

/// Apply the routers' [`SpellKitFx`] edges per unit in the order the packets raised them
/// ([`SpellKitFx::at`], the frame's own last): `Begin` records instances, `Reap` decays the
/// spell's persistent ones, so a GO's precast dies before its release flash. A display rebuild
/// this frame ([`crate::entities::DisplaySwapped::at`]) has already ended what the unit held; the
/// edges the wire put before it end there too, as the rebuild's loop `0x60ac10` stops what was
/// started before the display's watcher ran ([`Rebuild::Display`]), and those after it stand.
#[allow(clippy::too_many_arguments)] // one Bevy system's full input set
pub(super) fn resolve_spell_fx(
    mut commands: Commands,
    mut events: MessageReader<SpellKitFx>,
    mut swaps: MessageReader<crate::entities::DisplaySwapped>,
    mut units: Query<&mut FxAttached>,
    fx: Option<ResMut<SpellFx>>,
    time: Res<Time>,
    asset_server: Res<AssetServer>,
    mut emitters: Query<&mut benilla_world::particles::ParticleEmitter>,
) {
    let Some(mut fx) = fx else { return };
    let now = time.elapsed_secs();
    let mut rebuilt: EntityHashMap<crate::lanes::Stamp> = EntityHashMap::default();
    for swap in swaps.read() {
        if let Some(at) = swap.at {
            let latest = rebuilt.entry(swap.entity).or_insert(at);
            *latest = (*latest).max(at);
        }
    }
    let mut ops: EntityHashMap<Vec<&SpellKitFx>> = EntityHashMap::default();
    for ev in events.read() {
        ops.entry(ev.entity()).or_default().push(ev);
    }
    for (entity, mut edges) in ops {
        if commands.get_entity(entity).is_err() {
            continue;
        }
        let existing = units.get_mut(entity).ok();
        let mut instances = match existing {
            Some(mut att) => std::mem::take(&mut att.instances),
            None => Vec::new(),
        };
        // Wire order, stable within a packet; an edge the frame raised comes after every packet.
        edges.sort_by_key(|e| (e.at().is_none(), e.at()));
        let cut = rebuilt
            .get(&entity)
            .map(|&rebuild| edges.partition_point(|e| e.at().is_some_and(|at| at < rebuild)));
        let len = edges.len();
        for (i, edge) in edges.into_iter().enumerate() {
            if cut == Some(i) {
                end_for_rebuild(
                    &mut instances,
                    Rebuild::Display,
                    entity,
                    &mut commands,
                    &mut emitters,
                );
            }
            match edge {
                SpellKitFx::Begin {
                    spell_id,
                    persistent,
                    class,
                    stage,
                    effects,
                    ..
                } => {
                    // A persistent Begin first reaps the same (spell, class): a re-applied aura's
                    // old node decays out while the new one is born, as in the reference.
                    if *persistent {
                        reap_matching(
                            &mut instances,
                            *spell_id,
                            *class,
                            true,
                            &fx,
                            now,
                            &mut commands,
                            &mut emitters,
                        );
                    }
                    for FxSlot { tag, effect, path } in effects {
                        // Per slot, as `AddEffect 0x61fdd0` opens with `0x6208e0(owner, rec, tag)`.
                        replace_same_slot(
                            &mut instances,
                            *effect,
                            *tag,
                            &mut commands,
                            &mut emitters,
                        );
                        ensure_model(&mut fx, &asset_server, path);
                        instances.push(FxInstance {
                            spell_id: *spell_id,
                            persistent: *persistent,
                            class: *class,
                            stage: *stage,
                            tag: *tag,
                            effect: *effect,
                            path: path.clone(),
                            root: None,
                            expires: None,
                            decaying: false,
                        });
                    }
                }
                SpellKitFx::Reap {
                    spell_id,
                    class,
                    force,
                    ..
                } => {
                    reap_matching(
                        &mut instances,
                        *spell_id,
                        *class,
                        *force,
                        &fx,
                        now,
                        &mut commands,
                        &mut emitters,
                    );
                }
            }
        }
        if cut == Some(len) {
            end_for_rebuild(
                &mut instances,
                Rebuild::Display,
                entity,
                &mut commands,
                &mut emitters,
            );
        }
        match units.get_mut(entity) {
            Ok(mut att) => att.instances = instances,
            Err(_) => {
                commands.entity(entity).insert(FxAttached { instances });
            }
        }
    }
}

/// The teardown `0x6203e0` of a spawned instance: the root and its meshes leave the draw list at
/// once (`0x719207`), while its emitters drain, emission stopping and live particles ageing out,
/// as the reference gates the particle draw on the live count (`0x7b4b46`).
fn end_root(
    commands: &mut Commands,
    emitters: &mut Query<&mut benilla_world::particles::ParticleEmitter>,
    root: Entity,
) {
    for mut e in emitters.iter_mut() {
        if e.anchor() == Some(root) {
            e.drain_on_owner_loss();
        }
    }
    commands.entity(root).despawn();
}

/// Which rebuild of a unit's visual ends its instances, and so which come back on the new body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Rebuild {
    /// The reference's display rebuild `0x60abe0`. It stops every effect on the unit's list through
    /// `0x6203e0` (`0x60ac10`), a world plant too, which `0x61fcf0` links on the same list
    /// (`0x61fdbf`), then plays kind 2 again and nothing else that lives on: the aura slots' state
    /// kits (`0x5ff130` at `0x60ada6`) and the channel kit (`0x612a30` at `0x60adad`, kind 2 at
    /// `0x612b2b`). A [`FxStage::State`] instance re-arms; a precast loop, the loot sparkle (hung
    /// again only by `0x6005d0`'s own callers), a one-shot or a decaying one is dropped.
    Display,
    /// benilla's rig heal, a rebuild the reference never runs: what the unit had comes back, every
    /// persistent instance that is not decaying.
    Heal,
}

/// End `instances` for `rebuild`: each spawned root goes through the teardown, and only what the
/// rebuild brings back stays, pending its new body.
fn end_for_rebuild(
    instances: &mut Vec<FxInstance>,
    rebuild: Rebuild,
    unit: Entity,
    commands: &mut Commands,
    emitters: &mut Query<&mut benilla_world::particles::ParticleEmitter>,
) {
    instances.retain_mut(|inst| {
        if let Some(root) = inst.root.take() {
            end_root(commands, emitters, root);
        }
        let re_arm = inst.persistent
            && !inst.decaying
            && (rebuild == Rebuild::Heal || inst.stage == FxStage::State);
        if benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "fx",
                &format!(
                    "kit rebuild end unit={unit} path={} {rebuild:?} re_arm={re_arm}",
                    inst.path
                ),
            );
        }
        re_arm
    });
}

/// What a rebuild of a unit's visual needs to end the unit's instances first ([`Self::end_all`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct RebuildFx<'w, 's> {
    lists: Query<'w, 's, &'static mut FxAttached>,
    emitters: Query<'w, 's, &'static mut benilla_world::particles::ParticleEmitter>,
}

impl RebuildFx<'_, '_> {
    /// End `unit`'s instances for `rebuild` ([`end_for_rebuild`]). Called before the visual's
    /// despawn, so each root goes first and no later teardown meets a dead one.
    pub(super) fn end_all(&mut self, commands: &mut Commands, unit: Entity, rebuild: Rebuild) {
        let Ok(mut att) = self.lists.get_mut(unit) else {
            return;
        };
        end_for_rebuild(
            &mut att.instances,
            rebuild,
            unit,
            commands,
            &mut self.emitters,
        );
    }
}

/// The same-slot replace (`0x6208e0`): destroy each live node with the same `SpellVisualEffectName`
/// record at the same tag, without a decay, which keeps the effect count flat in the number of
/// attackers. The world-plant tag (-1) is exempt (`0x620913`); a decaying instance is out of reach.
fn replace_same_slot(
    instances: &mut Vec<FxInstance>,
    effect: u32,
    tag: u16,
    commands: &mut Commands,
    emitters: &mut Query<&mut benilla_world::particles::ParticleEmitter>,
) {
    if tag == benilla_formats::WORLD_EFFECT_TAG {
        return;
    }
    instances.retain(|i| {
        if i.decaying || i.effect != effect || i.tag != tag {
            return true;
        }
        if let Some(root) = i.root {
            end_root(commands, emitters, root);
        }
        false
    });
}

/// The spell-id reap (`0x614150`): each persistent instance of `(spell_id, class)` plays `Decay`
/// out, or goes at once without a model, a ready model or a `Decay` (`0x614187`–`0x6141a1`).
/// Unforced it takes only a looping kit's, kinds 3 and 4 ([`FxStage::Relive`]): kind 2's state
/// carries flag 0x1000 (`0x60eecd`), taken only when forced (`0x614179`), and kinds 0 and 1 carry
/// 0x20 (`0x60eeb5`), never taken (`0x614181`).
fn reap_matching(
    instances: &mut Vec<FxInstance>,
    spell_id: u32,
    class: FxClass,
    force: bool,
    fx: &SpellFx,
    now: f32,
    commands: &mut Commands,
    emitters: &mut Query<&mut benilla_world::particles::ParticleEmitter>,
) {
    instances.retain_mut(|i| {
        if i.decaying || !i.persistent || i.spell_id != spell_id || i.class != class {
            return true;
        }
        if !force && i.stage != FxStage::Relive {
            return true;
        }
        let span = i
            .root
            .and_then(|_| fx.models.get(&i.path))
            .and_then(|dm| decay_span(dm.animations.as_ref()));
        let (Some(root), Some(span)) = (i.root, span) else {
            if let Some(root) = i.root {
                end_root(commands, emitters, root); // destroyed at once
            }
            return false;
        };
        commands.entity(root).try_insert(FxDecay);
        i.decaying = true;
        i.expires = Some(now + span);
        lifecycle::trace_leg("decay", root, lifecycle::ANIM_DECAY);
        true
    });
}

/// Spawn pending instances whose model has built, and run the self-termination clock. A root hangs
/// under its attach joint and rides the bone; a world plant ([`WorldPlantFx`]) stands free.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(super) fn attach_spell_fx(
    mut commands: Commands,
    mut units: Query<(
        Entity,
        &mut FxAttached,
        Option<&BoneAttach>,
        Option<&mut benilla_world::rig_anim::RigPose>,
        &GlobalTransform,
        Has<crate::entities::VisualAttached>,
    )>,
    fx: Option<ResMut<SpellFx>>,
    asset_server: Res<AssetServer>,
    spells: Option<Res<crate::ui_action::Spells>>,
    time: Res<Time>,
    mut wow_materials: ResMut<Assets<WowModelMaterial>>,
    mut tint_reg: ResMut<FxTintAnims>,
    mut uv_reg: ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
    mut anim_table: ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    ibps: Res<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
    mut emitters: Query<&mut benilla_world::particles::ParticleEmitter>,
) {
    let Some(mut fx) = fx else {
        return;
    };
    let now = time.elapsed_secs();
    for (unit, mut att, bones, mut pose, unit_gt, body_built) in &mut units {
        att.instances.retain_mut(|inst| {
            // Self-termination (the cast-release flash ran its span).
            if let Some(expires) = inst.expires {
                if now >= expires {
                    if let Some(root) = inst.root {
                        end_root(&mut commands, &mut emitters, root);
                    }
                    if benilla_assets::trace::enabled() {
                        benilla_assets::trace::line(
                            "fx",
                            &format!("kit expire unit={unit} path={}", inst.path),
                        );
                    }
                    return false;
                }
            }
            // Spawned: only its own teardown or the rebuild's ([`RebuildFx`]) ends it.
            if inst.root.is_some() {
                return true;
            }
            // Pending: unspawnable flashes time out; a spawn sets the real span.
            if !inst.persistent && inst.expires.is_none() {
                inst.expires = Some(now + PENDING_TIMEOUT);
            }
            ensure_model(&mut fx, &asset_server, &inst.path);
            let Some(dm) = fx.models.get(&inst.path) else {
                return true; // unreachable: just inserted
            };
            if dm.parts.is_none() {
                return true; // model still loading
            }
            // The world-plant slot (kit field 12): no attach tag (`0x61fcf0` pushes -1), planted
            // once in world space (`0x620c86` via `0x620a90`), riding no bone.
            let planted = inst.tag == benilla_formats::WORLD_EFFECT_TAG;
            // A bone-riding effect waits for the body: before it is built there are no attach
            // points, and a live root is never re-seated. The reference re-arms after the rebuild
            // sets the model (`0x60abe0` → `0x5ff130`); the cube fallback counts as built.
            if !planted && !body_built {
                return true;
            }
            // Ground-anchored: a world plant, or an instance on the base point (`0x13`) or the unit
            // root. A chest-anchored one keeps its flat quads as geometry (`ProtectionFrom*`).
            let (root, ground_anchor) = if planted {
                let follow =
                    inst.persistent && spell_has_root_aura(spells.as_deref(), inst.spell_id);
                let root = commands
                    .spawn((
                        world_plant_transform(unit_gt),
                        Visibility::default(),
                        WorldPlantFx {
                            owner: unit,
                            follow,
                        },
                    ))
                    // Chain-only visibility: the wrapper draws nothing, its children do.
                    .vis_chain_only()
                    .id();
                (root, true)
            } else {
                // `CMissile`'s cascade: the slot's tag, `0xf`, `0x13`, else the unit root.
                let point = bones.and_then(|b| {
                    std::iter::once(inst.tag)
                        .chain(ATTACH_FALLBACKS)
                        .find_map(|tag| b.points.get(&tag).copied().map(|p| (tag, p)))
                        .and_then(|(tag, (bone, offset))| {
                            pose.as_mut()
                                .and_then(|p| p.anchor_for(&mut commands, unit, bone))
                                .map(|joint| (tag, joint, offset))
                        })
                });
                let ground_anchor = point.is_none_or(|(tag, ..)| tag == 0x13);
                let (parent, offset) = point.map_or((unit, Vec3::ZERO), |(_, j, o)| (j, o));
                let root = commands
                    .spawn((Transform::from_translation(offset), Visibility::default()))
                    // Chain-only, as above.
                    .vis_chain_only()
                    .id();
                commands.entity(parent).add_child(root);
                (root, ground_anchor)
            };
            // Parts were checked ready above, so this always attaches.
            attach_effect_visuals(
                &mut commands,
                root,
                dm,
                now,
                ground_anchor,
                // Chained to the unit: it fades and is freed with the body.
                EffectHost { parent: Some(unit) },
                // A kit effect is a `CEffect`: it runs the stage's lifecycle.
                Some(inst.stage),
                &mut FxMaterials {
                    store: &mut wow_materials,
                    tint: &mut tint_reg,
                    uv: &mut uv_reg,
                    table: &mut anim_table,
                },
                &ibps,
                &mut palettes,
                None, // an attach-point kit effect opens on its model's own `Stand`
            );
            inst.root = Some(root);
            if !inst.persistent {
                // One pass of sequence 0, from the raw sequence table: a sequence with no bone
                // keys builds no clip (the eat/drink tankard). A looping one counts as one pass;
                // the reference's loop boundary is untraced, but its tankard ends with the drink.
                let span = dm.first_seq_span.unwrap_or(FALLBACK_SPAN);
                inst.expires = Some(now + span);
            }
            if benilla_assets::trace::enabled() {
                benilla_assets::trace::line(
                    "fx",
                    &format!(
                        "kit spawn unit={unit} path={} persistent={} span={:?}",
                        inst.path,
                        inst.persistent,
                        inst.expires.map(|e| e - now)
                    ),
                );
            }
            true
        });
    }
}

/// Fire the event keyframes each instance's clip crossed since last frame ([`scan_events`]), at the
/// host unit, so a sound authored in the effect's own M2 plays. An instance is born at t = 0, so
/// first sight fires `[0, cur]`: the level-up pillar's `$SND(888)` sits at 0.033 s.
pub(super) fn fire_fx_anim_events(
    units: Query<(Entity, &FxAttached)>,
    players: Query<&AnimationPlayer>,
    // The instance root's world frame and rig, the frame its event records are authored in.
    globals: Query<&GlobalTransform>,
    poses: Query<&benilla_world::rig_anim::RigPose>,
    fx: Option<Res<SpellFx>>,
    mut last: Local<EntityHashMap<f32>>,
    mut seen: Local<Vec<Entity>>,
    mut out: MessageWriter<AnimSoundEvent>,
) {
    let Some(fx) = fx else { return };
    seen.clear();
    for (unit, att) in &units {
        for inst in &att.instances {
            let Some(root) = inst.root else { continue };
            let Some(clip) = fx
                .models
                .get(&inst.path)
                .and_then(|dm| dm.animations.as_ref())
                .and_then(|a| a.clips.first())
                .filter(|c| !c.events.is_empty())
            else {
                continue;
            };
            let Ok(player) = players.get(root) else {
                continue;
            };
            let Some(active) = player.animation(clip.node) else {
                continue;
            };
            let cur = active.seek_time();
            seen.push(root);
            let prev = last.insert(root, cur).unwrap_or(-1.0);
            // Fired at the unit, but the point composes through the root's frame, the space it is
            // authored in (`ArcaneShot_Area.m2` puts a `$SND` 30.5 yd out).
            let Ok(root_world) = globals.get(root) else {
                continue;
            };
            let frame = crate::creature_anim::EventFrame {
                world: root_world,
                rig: poses
                    .get(root)
                    .ok()
                    .and_then(|p| Some((p, globals.get(p.joints_root).ok()?))),
            };
            scan_events(clip, unit, prev, cur, &frame, &mut out);
        }
    }
    if last.len() > seen.len() {
        let live: bevy::ecs::entity::EntityHashSet = seen.iter().copied().collect();
        last.retain(|e, _| live.contains(e));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lanes::Stamp;
    use bevy::asset::AssetPlugin;
    use bevy::mesh::skinning::SkinnedMeshInverseBindposes;

    /// One spawned instance of Ice Barrier's state model.
    fn instance(root: Entity, persistent: bool) -> FxInstance {
        FxInstance {
            spell_id: 13033, // Ice Barrier
            persistent,
            class: FxClass::AuraState,
            stage: FxStage::State,
            tag: 0x13,
            effect: 1499, // Ice Barrier's state model
            path: "Spells\\IceShield_State.mdx".into(),
            root: Some(root),
            expires: None,
            decaying: false,
        }
    }

    /// An app running `attach_spell_fx`, and a unit with one spawned instance per `persistent`.
    fn standing(persistent: &[bool]) -> (App, Entity, Vec<Entity>) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<WowModelMaterial>()
            .init_asset::<benilla_assets::M2Model>()
            .init_asset::<SkinnedMeshInverseBindposes>()
            .init_resource::<SpellFx>()
            .init_resource::<FxTintAnims>()
            .init_resource::<benilla_world::doodad_anim::UvAnimMaterials>()
            .init_resource::<benilla_world::mat_anim_table::MatAnimTable>()
            .init_resource::<benilla_world::rig_palette::RigPalettes>()
            .add_systems(Update, attach_spell_fx);
        let roots: Vec<Entity> = persistent
            .iter()
            .map(|_| app.world_mut().spawn_empty().id())
            .collect();
        let instances = roots
            .iter()
            .zip(persistent)
            .map(|(&root, &p)| instance(root, p))
            .collect();
        let unit = app
            .world_mut()
            .spawn((
                Transform::default(),
                GlobalTransform::default(),
                FxAttached { instances },
            ))
            .id();
        (app, unit, roots)
    }

    fn instances_of(app: &App, unit: Entity) -> Vec<(bool, Option<Entity>)> {
        app.world()
            .entity(unit)
            .get::<FxAttached>()
            .unwrap()
            .instances
            .iter()
            .map(|i| (i.persistent, i.root))
            .collect()
    }

    // ---- the rebuild (`0x60abe0`): a display swap, and the rig heal that takes its shape ----

    /// The cast flash's record, tag and model: a one-shot release kit at a hand.
    const FLASH: (u32, u16, &str) = (7, 0x16, "Spells\\CastFlash.mdx");
    /// A one-shot planted at the unit's feet (kit field 12), standing free of its visual.
    const PLANT: (u32, u16, &str) = (8, benilla_formats::WORLD_EFFECT_TAG, "Spells\\Ring.mdx");

    /// The frame's rebuilds, kit starts and spawn pass in the plugin's order (its own
    /// `rebuild_then_start_kits`), on a clock the test advances.
    fn rebuilding() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins.build().disable::<bevy::time::TimePlugin>(),
            AssetPlugin::default(),
        ))
        .init_asset::<WowModelMaterial>()
        .init_asset::<benilla_assets::M2Model>()
        .init_asset::<SkinnedMeshInverseBindposes>()
        .init_resource::<Time>()
        .init_resource::<SpellFx>()
        .init_resource::<FxTintAnims>()
        .init_resource::<benilla_world::doodad_anim::UvAnimMaterials>()
        .init_resource::<benilla_world::mat_anim_table::MatAnimTable>()
        .init_resource::<benilla_world::rig_palette::RigPalettes>()
        .add_message::<SpellKitFx>()
        .add_message::<crate::entities::live_display::DisplaySwapped>()
        .add_message::<crate::entities::DisplayRebuilt>()
        .add_message::<crate::creature_anim::SpellKitSound>()
        .add_systems(
            Update,
            (crate::entities::rebuild_then_start_kits(), attach_spell_fx).chain(),
        );
        app
    }

    /// One emitter anchored on `root`, spawned `Free` as every attached instance's is.
    fn emitter_on(app: &mut App, root: Entity) -> Entity {
        let em = benilla_assets::ModelEmitter {
            def: benilla_world::testing::plain_particle_def(),
            texture: Some(Handle::default()),
            bone_pivot: [0.0; 3],
            billboard: None,
            recursion: None,
            geometry: None,
            owner_reach: 0.0,
            water_bound: (Vec3::ZERO, 0.0),
            idle_seq: 0,
        };
        let world = app.world_mut();
        let e = particles::spawn_emitter(
            &mut world.commands(),
            &em,
            Transform::IDENTITY,
            particles::EmitterFrames {
                owner: Some((root, [0.0; 3])),
                anchor: Some(root),
                on_owner_loss: particles::OwnerLoss::Free,
                alpha: Some(root),
                light_node: None,
            },
            particles::EmitClock::Effect(None),
        )
        .expect("an emitter");
        world.flush();
        e
    }

    /// A unit in display 7 with a built visual, `body` standing for its rig: a cast flash ending at
    /// 1 s and Ice Barrier's state hang under `body`, a ground plant stands free, and each root has
    /// one emitter. Returns the unit, the three roots and their emitters.
    fn mid_cast() -> (App, Entity, [Entity; 3], [Entity; 3]) {
        let mut app = rebuilding();
        let world = app.world_mut();
        let body = world
            .spawn((Transform::default(), GlobalTransform::default()))
            .id();
        let [flash_root, aura_root] = [(); 2].map(|_| {
            let root = world
                .spawn((Transform::default(), GlobalTransform::default()))
                .id();
            world.entity_mut(body).add_child(root);
            root
        });
        let plant_root = world.spawn(Transform::default()).id();
        let one_shot = |(effect, tag, path): (u32, u16, &str), root| FxInstance {
            spell_id: 116, // Frostbolt
            persistent: false,
            class: FxClass::Hold,
            stage: FxStage::OneShot,
            tag,
            effect,
            path: path.into(),
            root: Some(root),
            expires: Some(1.0),
            decaying: false,
        };
        let unit = built_unit(
            world,
            body,
            vec![
                one_shot(FLASH, flash_root),
                instance(aura_root, true),
                one_shot(PLANT, plant_root),
            ],
        );
        world.entity_mut(plant_root).insert(WorldPlantFx {
            owner: unit,
            follow: false,
        });
        let roots = [flash_root, aura_root, plant_root];
        let emitters = roots.map(|root| emitter_on(&mut app, root));
        (app, unit, roots, emitters)
    }

    /// A unit in display 7 whose visual is built, `body` standing for its rig, wearing `instances`.
    fn built_unit(world: &mut World, body: Entity, instances: Vec<FxInstance>) -> Entity {
        world
            .spawn((
                Transform::default(),
                GlobalTransform::default(),
                crate::entities::VisualAttached,
                crate::net::Guid(0xB0B),
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: Some(7),
                    scale: 1.0,
                },
                display_field(7),
                crate::entities::live_display::AppliedDisplay(Some(7)),
                FxAttached { instances },
            ))
            .add_child(body)
            .id()
    }

    /// A built unit wearing `kits`, each spawned on a root under its visual, every model ready.
    fn wearing(kits: Vec<FxInstance>) -> (App, Entity) {
        let mut app = rebuilding();
        let world = app.world_mut();
        for kit in &kits {
            world.resource_mut::<SpellFx>().models.insert(
                kit.path.clone(),
                DisplayModel {
                    parts: Some(Vec::new()),
                    ..crate::entities::display::empty_shell()
                },
            );
        }
        let body = world
            .spawn((Transform::default(), GlobalTransform::default()))
            .id();
        let kits = kits
            .into_iter()
            .map(|mut kit| {
                let root = world
                    .spawn((Transform::default(), GlobalTransform::default()))
                    .id();
                world.entity_mut(body).add_child(root);
                kit.root = Some(root);
                kit
            })
            .collect();
        let unit = built_unit(world, body, kits);
        (app, unit)
    }

    /// Swap the display, then build the new body, as `attach_entity_visuals` would a frame on.
    fn swap_then_build(app: &mut App, unit: Entity) {
        swap_display(app, unit);
        app.update();
        assert_names_no_dead_root(app, unit);
        app.world_mut()
            .entity_mut(unit)
            .insert(crate::entities::VisualAttached);
        app.update();
        app.update();
        assert_names_no_dead_root(app, unit);
    }

    /// A persistent hand kit of `spell_id` running `stage`.
    fn held(spell_id: u32, stage: FxStage, effect: u32, path: &str) -> FxInstance {
        FxInstance {
            spell_id,
            persistent: true,
            class: FxClass::Hold,
            stage,
            tag: 0x15,
            effect,
            path: path.into(),
            root: None,
            expires: None,
            decaying: false,
        }
    }

    fn roots_of(app: &App, unit: Entity) -> Vec<(u32, Option<Entity>)> {
        app.world()
            .get::<FxAttached>(unit)
            .unwrap()
            .instances
            .iter()
            .map(|i| (i.spell_id, i.root))
            .collect()
    }

    /// The rebuild replays the aura slots (`0x5ff130` at `0x60ada6`) and the channel kit
    /// (`0x612a30` at `0x60adad`, kind 2 at `0x612b2b`), so both come back on the new body, each
    /// on a fresh root.
    #[test]
    fn the_rebuild_replays_the_aura_and_the_channel_on_the_new_body() {
        let channel = held(5143, FxStage::State, 9, "Spells\\Channel_Hand.mdx"); // Arcane Missiles
        let (mut app, unit) = wearing(vec![instance(Entity::PLACEHOLDER, true), channel]);
        let before = roots_of(&app, unit);
        swap_then_build(&mut app, unit);
        let after = roots_of(&app, unit);
        assert_eq!(
            after.iter().map(|&(spell, _)| spell).collect::<Vec<_>>(),
            [13033, 5143],
            "the aura and the channel come back"
        );
        for ((_, old), (spell, new)) in before.into_iter().zip(after) {
            assert!(
                new.is_some() && new != old,
                "spell {spell} spawns afresh on the new body"
            );
        }
    }

    /// The rebuild runs on the unit, built or not (`0x60abe0`): with its body still to come, what
    /// it held ends all the same, the aura re-arming for the body and the precast loop and the
    /// flash gone.
    #[test]
    fn a_rebuild_before_the_body_attached_ends_what_the_unit_held() {
        let mut app = rebuilding();
        let flash = FxInstance {
            persistent: false,
            ..held(7, FxStage::OneShot, FLASH.0, FLASH.2)
        };
        let precast = held(116, FxStage::Relive, 10, "Spells\\Precast_Hand.mdx");
        let aura = held(13033, FxStage::State, 9, "Spells\\Aura.mdx");
        let unit = app
            .world_mut()
            .spawn((
                Transform::default(),
                GlobalTransform::default(),
                crate::net::Guid(0xB0B),
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: Some(7),
                    scale: 1.0,
                },
                display_field(7),
                FxAttached {
                    instances: vec![flash, precast, aura],
                },
            ))
            .id();
        swap_display(&mut app, unit);
        app.update();
        assert_eq!(
            roots_of(&app, unit),
            [(13033, None)],
            "the aura waits for the body"
        );
    }

    /// A precast loop is kind 4 (`0x60ee7e`), which nothing in the rebuild plays again: it ends
    /// and stays gone.
    #[test]
    fn a_precast_loop_ends_at_the_rebuild_and_stays_gone() {
        let precast = held(116, FxStage::Relive, 10, "Spells\\Precast_Hand.mdx"); // Frostbolt
        let (mut app, unit) = wearing(vec![precast]);
        swap_then_build(&mut app, unit);
        assert_eq!(instances_of(&app, unit), vec![]);
    }

    /// The loot sparkle (`0x6005d0`) is hung only by its own triggers: the death stamp (`0x5fbe6d`,
    /// `0x5fc962`), a dynamic-flags write (`0x60058e`), a return (`0x5fbde2`) or a create
    /// (`0x5fba33`). The rebuild reaches none of them, and the new model plays no death to finish,
    /// as a dead unit's base pick is 6 or 132 (`0x5fcff0`): the sparkle ends and stays gone.
    #[test]
    fn the_loot_sparkle_ends_at_the_rebuild_and_stays_gone() {
        let sparkle = FxInstance {
            spell_id: u32::MAX, // the router's loot key
            tag: 0x13,
            ..held(0, FxStage::Relive, 11, "Spells\\LootSparkle.mdx")
        };
        let (mut app, unit) = wearing(vec![sparkle]);
        swap_then_build(&mut app, unit);
        assert_eq!(instances_of(&app, unit), vec![]);
    }

    /// The rig heal is benilla's own: the reference never rebuilds there, so everything the unit
    /// had comes back on the healed body, the precast loop and the loot sparkle too, which the
    /// loot router hangs only on its rising edge and so would never hang again.
    #[test]
    fn a_rig_heal_brings_back_every_persistent_effect() {
        let precast = held(116, FxStage::Relive, 10, "Spells\\Precast_Hand.mdx"); // Frostbolt
        let sparkle = FxInstance {
            spell_id: u32::MAX, // the router's loot key
            tag: 0x13,
            ..held(0, FxStage::Relive, 11, "Spells\\LootSparkle.mdx")
        };
        let (mut app, unit) = wearing(vec![instance(Entity::PLACEHOLDER, true), precast, sparkle]);
        let before = roots_of(&app, unit);
        app.world_mut()
            .entity_mut(unit)
            .insert(benilla_world::rig_palette::RigStarved);
        app.update();
        assert_names_no_dead_root(&app, unit);
        app.world_mut()
            .entity_mut(unit)
            .insert(crate::entities::VisualAttached);
        app.update();
        let after = roots_of(&app, unit);
        assert_eq!(
            after.iter().map(|&(spell, _)| spell).collect::<Vec<_>>(),
            [13033, 116, u32::MAX],
            "the aura, the precast loop and the sparkle all come back"
        );
        for ((_, old), (spell, new)) in before.into_iter().zip(after) {
            assert!(
                new.is_some() && new != old,
                "spell {spell} spawns afresh on the healed body"
            );
        }
    }

    /// Write `starts` as this frame's kit edges, beside a display rebuild at `rebuild`.
    fn swap_with(app: &mut App, unit: Entity, starts: Vec<SpellKitFx>, rebuild: Stamp) {
        let mut messages = app.world_mut().resource_mut::<Messages<SpellKitFx>>();
        for start in starts {
            messages.write(start);
        }
        swap_display_at(app, unit, rebuild);
    }

    /// `n` places in wire order.
    fn places<const N: usize>() -> [Stamp; N] {
        [(); N].map(|_| crate::lanes::stamp())
    }

    /// A start of one model at `tag`, ready to spawn.
    fn start(
        app: &mut App,
        unit: Entity,
        (spell_id, persistent, stage): (u32, bool, FxStage),
        (effect, tag, path): (u32, u16, &str),
        at: Option<Stamp>,
    ) -> SpellKitFx {
        app.world_mut().resource_mut::<SpellFx>().models.insert(
            path.into(),
            DisplayModel {
                parts: Some(Vec::new()),
                ..crate::entities::display::empty_shell()
            },
        );
        SpellKitFx::Begin {
            entity: unit,
            spell_id,
            persistent,
            class: if stage == FxStage::State {
                FxClass::AuraState
            } else {
                FxClass::Hold
            },
            stage,
            effects: vec![FxSlot {
                tag,
                effect,
                path: path.into(),
            }],
            at,
        }
    }

    fn paths_of(app: &App, unit: Entity) -> Vec<(String, bool)> {
        app.world()
            .get::<FxAttached>(unit)
            .unwrap()
            .instances
            .iter()
            .map(|i| (i.path.clone(), i.root.is_some()))
            .collect()
    }

    /// Build the new body a frame on, then let the spawn pass run.
    fn build_body(app: &mut App, unit: Entity) {
        app.update();
        assert_names_no_dead_root(app, unit);
        app.world_mut()
            .entity_mut(unit)
            .insert(crate::entities::VisualAttached);
        app.update();
        assert_names_no_dead_root(app, unit);
    }

    /// A unit that dies in a form leaves the form and turns lootable in one update. The display
    /// watcher (131, `0x604990`) rebuilds before the dynamic flags' (143, `0x604b30`), whose
    /// sparkle (`0x60058e`), or the one the death's end hangs (`0x5fc962`), comes after it.
    /// So the sparkle starts on the new body and stays.
    #[test]
    fn a_unit_that_dies_in_a_form_and_turns_lootable_in_one_update_keeps_its_sparkle() {
        let (mut app, unit) = wearing(vec![]);
        let [rebuild, flags] = places();
        let sparkle = start(
            &mut app,
            unit,
            (u32::MAX, true, FxStage::Relive), // as the loot router writes it
            (11, 0x13, "Spells\\LootSparkle.mdx"),
            Some(flags),
        );
        swap_with(&mut app, unit, vec![sparkle], rebuild);
        build_body(&mut app, unit);
        assert!(
            matches!(roots_of(&app, unit)[..], [(u32::MAX, Some(_))]),
            "the sparkle hangs on the new body: {:?}",
            roots_of(&app, unit)
        );
    }

    /// A form's aura added in the update that swaps the display: the aura watcher (fields 47 to
    /// 100, `0x604d00`) starts its state kit before the rebuild, which stops it and replays the
    /// slot (`0x5ff130`). One instance, on the new body.
    #[test]
    fn an_aura_added_with_a_swap_lands_once_on_the_new_body() {
        let (mut app, unit) = wearing(vec![]);
        let [aura_watch, rebuild] = places();
        let aura = start(
            &mut app,
            unit,
            (13033, true, FxStage::State),
            (1499, 0x13, "Spells\\IceShield_State.mdx"),
            Some(aura_watch),
        );
        swap_with(&mut app, unit, vec![aura], rebuild);
        build_body(&mut app, unit);
        assert!(
            matches!(roots_of(&app, unit)[..], [(13033, Some(_))]),
            "one aura instance, spawned on the new body: {:?}",
            roots_of(&app, unit)
        );
    }

    /// One update and the packets before it, as 1.12.1 orders them: the fields are written first
    /// and their watchers run after in field order, so the display's rebuild (131, `0x604990`)
    /// stops at `0x60ac10` what was started before it: a precast loop from an earlier
    /// `SMSG_SPELL_START`, the cast kit of the `SMSG_SPELL_GO` a shapeshift's update follows, a
    /// level-up ding (34, `0x6045b0`). What the mount (133, `0x604570`) and dynamic flags (143,
    /// `0x604b30`) watchers start after it stands, as does what the frame starts after every
    /// packet, a breath puff.
    #[test]
    fn a_rebuild_ends_the_starts_the_wire_put_before_it_and_keeps_those_after() {
        let (mut app, unit) = wearing(vec![]);
        let [spell_start, spell_go, level, rebuild, mount, flags] = places();
        let starts = vec![
            start(
                &mut app,
                unit,
                (116, true, FxStage::Relive),
                (10, 0x15, "Spells\\Precast_Hand.mdx"),
                Some(spell_start),
            ),
            start(
                &mut app,
                unit,
                (768, false, FxStage::OneShot), // Cat Form
                (7, 0x16, "Spells\\CastFlash.mdx"),
                Some(spell_go),
            ),
            start(
                &mut app,
                unit,
                (0, false, FxStage::OneShot),
                (12, 0x13, "Spells\\LevelUp.mdx"),
                Some(level),
            ),
            start(
                &mut app,
                unit,
                (0, false, FxStage::OneShot),
                (13, 0x13, "Spells\\MountPoof.mdx"),
                Some(mount),
            ),
            start(
                &mut app,
                unit,
                (u32::MAX, true, FxStage::Relive),
                (11, 0x13, "Spells\\LootSparkle.mdx"),
                Some(flags),
            ),
            start(
                &mut app,
                unit,
                (0, false, FxStage::OneShot),
                (14, 0x11, "Spells\\Breath.mdx"),
                None,
            ),
        ];
        swap_with(&mut app, unit, starts, rebuild);
        build_body(&mut app, unit);
        assert_eq!(
            paths_of(&app, unit),
            [
                ("Spells\\MountPoof.mdx".to_string(), true),
                ("Spells\\LootSparkle.mdx".to_string(), true),
                ("Spells\\Breath.mdx".to_string(), true),
            ],
            "the precast loop, the cast kit and the ding end; the rest spawn on the new body",
        );
    }

    /// A new display naming new records, whose rebuild the update announces (`0x60abe0`).
    fn swap_display(app: &mut App, unit: Entity) {
        swap_display_at(app, unit, crate::lanes::stamp());
    }

    /// [`swap_display`] with the rebuild at `at` among the packets' edges.
    fn swap_display_at(app: &mut App, unit: Entity, at: Stamp) {
        app.world_mut().entity_mut(unit).insert(display_field(8));
        app.world_mut()
            .resource_mut::<Messages<crate::entities::DisplayRebuilt>>()
            .write(crate::entities::DisplayRebuilt {
                entity: unit,
                at,
                display_id: 8,
            });
    }

    fn display_field(display: u32) -> crate::net::ObjectStore {
        crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_UNIT_DISPLAYID,
            display,
        )]))
    }

    /// Every root the list names is live. Each teardown site despawns only a named root, so a list
    /// naming a dead one is a despawn of a dead entity waiting for the next reap, replace or expiry.
    fn assert_names_no_dead_root(app: &App, unit: Entity) {
        for inst in &app.world().get::<FxAttached>(unit).unwrap().instances {
            if let Some(root) = inst.root {
                assert!(
                    app.world().get_entity(root).is_ok(),
                    "{} still names its root {root}, which is already despawned",
                    inst.path
                );
            }
        }
    }

    /// The rebuild's effect leg, as `0x60abe0` runs it: the `0x60ac10` loop stops every effect on
    /// the unit's list `+0xb4` through `0x6203e0` (`0x61fcf0` links a planted one there too,
    /// `0x61fdbf`), so each root goes and each emitter drains; the aura's state kit re-arms for the
    /// new body (`0x5ff130`). Then the flash's expiry, a forced reap of the aura and a replay of the
    /// flash's slot all run with nothing dead to despawn.
    fn assert_the_rebuild_ended_every_effect(
        mut app: App,
        unit: Entity,
        roots: [Entity; 3],
        emitters: [Entity; 3],
    ) {
        assert_names_no_dead_root(&app, unit);
        assert_eq!(
            instances_of(&app, unit),
            vec![(true, None)],
            "the flash and the plant end; the aura's state re-arms, pending the new body",
        );
        for (root, emitter) in roots.into_iter().zip(emitters) {
            assert!(app.world().get_entity(root).is_err(), "{root} ends");
            assert_eq!(
                app.world()
                    .get::<particles::ParticleEmitter>(emitter)
                    .expect("a stopped instance's emitter outlives its root")
                    .on_owner_loss(),
                particles::OwnerLoss::Drain,
                "the particles finish their lives (`0x61f680`), not pop",
            );
        }

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs(2));
        let mut messages = app.world_mut().resource_mut::<Messages<SpellKitFx>>();
        messages.write(SpellKitFx::Reap {
            entity: unit,
            spell_id: 13033,
            class: FxClass::AuraState,
            force: true,
            at: None,
        });
        messages.write(begin(unit, FLASH.0, FLASH.1, FLASH.2));
        app.update();
        assert_names_no_dead_root(&app, unit);
        assert_eq!(
            instances_of(&app, unit),
            vec![(false, None)],
            "the reap takes the pending aura; the replayed flash waits for its model",
        );
    }

    #[test]
    fn a_display_swap_ends_the_units_effects_before_its_visual_goes() {
        let (mut app, unit, roots, emitters) = mid_cast();
        swap_display(&mut app, unit);
        app.update();
        assert!(
            !app.world()
                .entity(unit)
                .contains::<crate::entities::VisualAttached>(),
            "the swap tore the visual down"
        );
        assert_the_rebuild_ended_every_effect(app, unit, roots, emitters);
    }

    /// The rig heal rebuilds through the display swap's teardown, so it ends the effects the same way.
    #[test]
    fn a_rig_heal_ends_the_units_effects_before_its_visual_goes() {
        let (mut app, unit, roots, emitters) = mid_cast();
        app.world_mut()
            .entity_mut(unit)
            .insert(benilla_world::rig_palette::RigStarved);
        app.update();
        assert!(
            !app.world()
                .entity(unit)
                .contains::<crate::entities::VisualAttached>(),
            "the heal tore the visual down"
        );
        assert_the_rebuild_ended_every_effect(app, unit, roots, emitters);
    }

    /// With the new body still loading, a re-armed instance waits rather than spawn at the root.
    #[test]
    fn a_re_armed_instance_waits_for_the_body_before_it_spawns() {
        let (mut app, unit, ..) = mid_cast();
        app.world_mut().resource_mut::<SpellFx>().models.insert(
            "Spells\\IceShield_State.mdx".into(),
            DisplayModel {
                parts: Some(Vec::new()),
                ..crate::entities::display::empty_shell()
            },
        );
        swap_display(&mut app, unit);
        app.update();
        app.update();
        assert_eq!(
            instances_of(&app, unit),
            vec![(true, None)],
            "no body yet: the instance holds, it does not spawn at the root"
        );
        app.world_mut()
            .entity_mut(unit)
            .insert(crate::entities::VisualAttached);
        app.update();
        assert!(
            matches!(instances_of(&app, unit)[..], [(true, Some(_))]),
            "the body is built: the instance spawns"
        );
    }

    /// A decaying instance was already ending, so the rebuild takes it too.
    #[test]
    fn a_decaying_instance_ends_at_the_rebuild() {
        let (mut app, unit, ..) = mid_cast();
        app.world_mut()
            .entity_mut(unit)
            .get_mut::<FxAttached>()
            .unwrap()
            .instances[1]
            .decaying = true;
        swap_display(&mut app, unit);
        app.update();
        assert_names_no_dead_root(&app, unit);
        assert_eq!(instances_of(&app, unit), vec![]);
    }

    #[test]
    fn a_live_root_is_never_disturbed() {
        let (mut app, unit, roots) = standing(&[true]);
        app.update();
        assert_eq!(instances_of(&app, unit), vec![(true, Some(roots[0]))]);
    }

    // ---- the same-slot replace (`0x6208e0`) ----

    /// The blood spurt's record and the two flank tags.
    const SPURT: u32 = 63;
    const FRONT: u16 = 0xf;
    const BACK: u16 = 0x10;

    /// An app running `resolve_spell_fx` alone, plus one bare unit.
    fn resolving() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<WowModelMaterial>()
            .init_asset::<benilla_assets::M2Model>()
            .init_resource::<SpellFx>()
            .add_message::<SpellKitFx>()
            .add_message::<crate::entities::DisplaySwapped>()
            .add_systems(Update, resolve_spell_fx);
        let unit = app.world_mut().spawn_empty().id();
        (app, unit)
    }

    /// One self-terminating `Begin` carrying one slot.
    fn begin(entity: Entity, effect: u32, tag: u16, path: &str) -> SpellKitFx {
        SpellKitFx::Begin {
            entity,
            spell_id: 0,
            persistent: false,
            class: FxClass::Hold,
            stage: FxStage::OneShot,
            effects: vec![FxSlot {
                tag,
                effect,
                path: path.into(),
            }],
            at: None,
        }
    }

    fn send(app: &mut App, msg: SpellKitFx) {
        app.world_mut()
            .resource_mut::<Messages<SpellKitFx>>()
            .write(msg);
        app.update();
    }

    fn slots_of(app: &App, unit: Entity) -> Vec<(u32, u16)> {
        app.world()
            .entity(unit)
            .get::<FxAttached>()
            .map(|a| a.instances.iter().map(|i| (i.effect, i.tag)).collect())
            .unwrap_or_default()
    }

    /// `0x6208e0`: a second play of the same record at the same tag destroys the first.
    #[test]
    fn a_replay_of_the_same_record_at_the_same_tag_replaces_rather_than_stacks() {
        let (mut app, unit) = resolving();
        send(&mut app, begin(unit, SPURT, FRONT, "Particles\\Spurt.mdx"));
        // The first instance has been attached: give it the root the spawn pass would.
        let root = app.world_mut().spawn_empty().id();
        app.world_mut()
            .entity_mut(unit)
            .get_mut::<FxAttached>()
            .unwrap()
            .instances[0]
            .root = Some(root);

        send(&mut app, begin(unit, SPURT, FRONT, "Particles\\Spurt.mdx"));
        assert_eq!(
            slots_of(&app, unit),
            vec![(SPURT, FRONT)],
            "one instance, not two"
        );
        assert!(
            app.world().get_entity(root).is_err(),
            "the replaced node is DESTROYED the same frame, not left to decay",
        );
    }

    /// Execute's cast kit hangs one record on both hands (`0x15` and `0x16`), and both survive.
    #[test]
    fn the_same_record_at_a_different_tag_coexists() {
        let (mut app, unit) = resolving();
        send(&mut app, begin(unit, SPURT, FRONT, "Particles\\Spurt.mdx"));
        send(&mut app, begin(unit, SPURT, BACK, "Particles\\Spurt.mdx"));
        assert_eq!(slots_of(&app, unit), vec![(SPURT, FRONT), (SPURT, BACK)]);
    }

    /// The key is the record, not the model path: two rows may name one `.mdx`.
    #[test]
    fn a_different_record_at_the_same_tag_coexists() {
        let (mut app, unit) = resolving();
        send(&mut app, begin(unit, SPURT, FRONT, "Particles\\Spurt.mdx"));
        send(
            &mut app,
            begin(unit, SPURT + 1, FRONT, "Particles\\Spurt.mdx"),
        );
        assert_eq!(
            slots_of(&app, unit),
            vec![(SPURT, FRONT), (SPURT + 1, FRONT)]
        );
    }

    /// `0x614150(spell, force)` never takes flag 0x20, kinds 0 and 1 (`0x614181`), and takes flag
    /// 0x1000, kind 2's state (`0x60eecd`), only when forced (`0x614179`): a cast edge's reap ends
    /// the spell's precast loop (kind 4, `0x60ee7e`) and spares its channel kit, which the channel's
    /// own forced stop takes (`0x60d22b`).
    #[test]
    fn an_unforced_reap_spares_the_spells_state_effects() {
        let (mut app, unit) = resolving();
        let (looping, state) = (
            app.world_mut().spawn_empty().id(),
            app.world_mut().spawn_empty().id(),
        );
        let hold = |root, stage| FxInstance {
            class: FxClass::Hold,
            stage,
            ..instance(root, true)
        };
        app.world_mut().entity_mut(unit).insert(FxAttached {
            instances: vec![hold(looping, FxStage::Relive), hold(state, FxStage::State)],
        });
        let reap = |force| SpellKitFx::Reap {
            entity: unit,
            spell_id: 13033,
            class: FxClass::Hold,
            force,
            at: None,
        };
        let roots = |app: &App| {
            app.world()
                .entity(unit)
                .get::<FxAttached>()
                .unwrap()
                .instances
                .iter()
                .map(|i| i.root)
                .collect::<Vec<_>>()
        };
        send(&mut app, reap(false));
        assert_eq!(roots(&app), [Some(state)], "the loop ends, the state stays");
        assert!(app.world().get_entity(looping).is_err());
        send(&mut app, reap(true));
        assert!(roots(&app).is_empty(), "forced, the state goes too");
    }

    /// The walk skips tag -1 (`0x620913`): a second Thunder Clap ring does not eat the first.
    #[test]
    fn the_world_plant_slot_is_excluded_from_the_replace() {
        let (mut app, unit) = resolving();
        let tag = benilla_formats::WORLD_EFFECT_TAG;
        send(&mut app, begin(unit, SPURT, tag, "Spells\\Ring.mdx"));
        send(&mut app, begin(unit, SPURT, tag, "Spells\\Ring.mdx"));
        assert_eq!(slots_of(&app, unit), vec![(SPURT, tag), (SPURT, tag)]);
    }

    /// A map change clears the cache: the spawn pass rebuilds the entry and keeps the instance.
    #[test]
    fn an_evicted_cache_entry_is_rebuilt_by_the_spawn_pass() {
        const PATH: &str = "Spells\\StunSwirl_State_Head.mdx";

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins.build().disable::<bevy::time::TimePlugin>(),
            AssetPlugin::default(),
        ))
        .init_asset::<WowModelMaterial>()
        .init_asset::<benilla_assets::M2Model>()
        .init_asset::<bevy::mesh::skinning::SkinnedMeshInverseBindposes>()
        .init_resource::<Time>()
        .init_resource::<SpellFx>()
        .init_resource::<FxTintAnims>()
        .init_resource::<benilla_world::doodad_anim::UvAnimMaterials>()
        .init_resource::<benilla_world::mat_anim_table::MatAnimTable>()
        .init_resource::<benilla_world::rig_palette::RigPalettes>()
        .add_systems(Update, attach_spell_fx);

        let unit = app
            .world_mut()
            .spawn((
                GlobalTransform::default(),
                FxAttached {
                    instances: vec![FxInstance {
                        spell_id: 9032,
                        persistent: true,
                        class: FxClass::AuraState,
                        stage: FxStage::State,
                        tag: 0x14,
                        effect: 50,
                        path: PATH.to_string(),
                        root: None,
                        expires: None,
                        decaying: false,
                    }],
                },
            ))
            .id();
        assert!(
            app.world().resource::<SpellFx>().models.is_empty(),
            "the fixture is a cache the eviction already cleared"
        );

        app.update();

        assert!(
            app.world().resource::<SpellFx>().models.contains_key(PATH),
            "the spawn pass must get-or-insert its entry, not read and give up"
        );
        assert_eq!(
            app.world()
                .entity(unit)
                .get::<FxAttached>()
                .expect("the unit keeps its instance list")
                .instances
                .len(),
            1,
            "the instance must survive to use the rebuilt entry — a persistent one is never reaped \
             for being pending"
        );
    }
}
