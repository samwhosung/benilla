//! Attaching a visual to each streamed entity once its shared display model has loaded: the
//! dressed parts, the rig and sequence clock, the character look ([`char_skin`]), emitters,
//! lights, ribbons, GameObject collision, or a debug cube for a display that names no model.

use avian3d::prelude::RigidBody;
use benilla_assets::ModelAnimations;
use benilla_protocol::EntityKind;
use bevy::prelude::*;

use benilla_assets::WorldAssets;
use bevy::animation::transition::AnimationTransitions;

use crate::creature_anim::AnimDriver;
use crate::net::{NetEntity, ObjectStore};
use crate::player::CameraPivot;
use crate::target::SelectionRadius;
use benilla_world::interact::WorldObject;
use benilla_world::model_fade::JoinedFade;
use benilla_world::model_render::ModelKind;
use benilla_world::particles;

use super::{
    Characters, Creatures, CubeAssets, DisplayModel, GameObjects, ModelHandle, SkinComposites,
    SkinSections, VisualAttached,
};

mod char_skin;
use char_skin::{build_char_skin_materials, equip_geosets, resolve_char_look, resolve_worn_equip};
mod dress;
mod merge;
use dress::{spawn_group, PartDress};
pub(super) use merge::MergedFormsCache;
mod preview;
pub(crate) use preview::equip_slot;
pub(super) use preview::{build_dressup_preview, build_glue_pet, build_glue_preview};
mod redress;

/// One body's drawn parts as `WOW_DRESS_CENSUS` prints them: index, merge group, blend, material.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct BodyPartsDesc<'w, 's> {
    parts: Query<
        'w,
        's,
        (
            &'static dress::DressedPart,
            Option<&'static merge::DressedGroup>,
            &'static benilla_world::model_render::ModelPart,
            &'static MeshMaterial3d<benilla_assets::materials::WowModelMaterial>,
        ),
    >,
}

impl BodyPartsDesc<'_, '_> {
    /// One line per drawn part under `unit`, sorted by part index.
    pub(crate) fn describe(&self, unit: Entity, children: &Query<&Children>) -> Vec<String> {
        let mut rows: Vec<(u32, String)> = children
            .iter_descendants(unit)
            .filter_map(|e| self.parts.get(e).ok())
            .map(|(part, group, model, mat)| {
                let group = group
                    .map(|g| format!("{:?}", &g.0[..]))
                    .unwrap_or_else(|| "-".into());
                (
                    part.index,
                    format!(
                        "DRESS_PART #{:<3} group={group:<14} blend={:?} mat={:?}",
                        part.index,
                        model.blend,
                        mat.0.id()
                    ),
                )
            })
            .collect();
        rows.sort_by_key(|(i, _)| *i);
        rows.into_iter().map(|(_, l)| l).collect()
    }
}
pub(super) use redress::redress_player_looks;

/// Set up the skinned instance of a creature, player body or animated GameObject: the pose
/// buffer, the palette slot and the global-sequence drive; the sequence clock is
/// [`arm_sequence_clock`]'s. No joint or anchor entities are spawned: a bone's anchor comes from
/// `RigPose::anchor_for` on first use. `None` when the model has no inverse bindposes; slot 0
/// means no palette slot, so parts draw the static bind-pose mesh. `skins` is false for a body
/// that draws nothing of its own (an invisible holder of a visible weapon), which still needs the
/// pose for its attachment points but takes no slot.
fn setup_skinned_instance(
    commands: &mut Commands,
    palettes: &mut benilla_world::rig_palette::RigPalettes,
    entity: Entity,
    joints_root: Entity,
    d: &DisplayModel,
    skins: bool,
) -> Option<RigBuild> {
    let ibp = d.inverse_bindposes.as_ref()?;
    let nbones = d.skeleton.joints.len();
    // `joints_root` is the rig's model-space frame: `entity`, a mounted rider's seat anchor or a
    // conform node, while the `AnimationPlayer` and driver stay on `entity`.
    let pose = benilla_world::rig_anim::RigPose::new(joints_root, &d.skeleton);
    // The on-replace hook frees the palette slot with the visual.
    let slot = match skins
        .then(|| {
            benilla_world::rig_palette::RigSkin::allocate_bones(
                palettes,
                nbones as u32,
                ibp.clone(),
            )
        })
        .flatten()
    {
        Some(rig) => {
            let slot = rig.slot;
            // `RigStarved` means slot-less: a rebuild that lands a slot clears it, or the healer
            // loops.
            commands
                .entity(entity)
                .insert(rig)
                .remove::<benilla_world::rig_palette::RigStarved>();
            slot
        }
        // Nothing to skin: no starvation marker, or `heal_rig_starved` would rebuild the body for a
        // slot it never asks for.
        None if !skins => 0,
        None => {
            // Table full: static bind-pose parts until `heal_rig_starved` rebuilds with a slot.
            commands
                .entity(entity)
                .insert(benilla_world::rig_palette::RigStarved);
            0
        }
    };
    if let Some(anims) = d.animations.as_ref() {
        // Global-sequence bone channels (an eyelid's blink) run on their own clock; they write
        // bone slots, so unlike the sequence clock they need the rig.
        if let Some(drive) =
            benilla_world::rig_anim::GlobalSeqDrive::new_rig(&anims.global_bones, nbones)
        {
            commands.entity(entity).insert(drive);
        }
    }
    Some(RigBuild { pose, slot })
}

/// Arm this instance's sequence clock: the `AnimationPlayer`, graph and [`ModelAnimations`], and
/// the driver that picks the sequence ([`crate::go_anim::GoAnim`] for a state GameObject,
/// `AnimDriver` for a unit). Armed with or without a rig: emitters, material loops and the
/// GameObject state arm read the playing sequence even on a model whose bones never move.
fn arm_sequence_clock(
    commands: &mut Commands,
    entity: Entity,
    anims: &ModelAnimations,
    kind: EntityKind,
    go_state_machine: bool,
) {
    // Every GameObject gets the loader-idle seed, which the reference arms the moment the M2 goes
    // live (`0x70ebd0`); a door's object-layer arm (`0x5f3930`) lands after it. Seeding first
    // spares a state GO a bind-pose first frame, and playing the seed through the transitions
    // lets the first arm fade out of it rather than leave two clips live.
    let mut player = AnimationPlayer::default();
    let mut transitions = AnimationTransitions::new();
    if kind == EntityKind::GameObject {
        // `idle_clip`, not `first_seq`: a GameObject whose sequences pose no bone has no
        // `first_seq`, but its emitters still read the clock.
        if let Some(clip) = anims.idle_clip() {
            // Loop iff `M2Sequence.flags` bit 0 is clear; a set bit plays the window once and
            // holds at its end (`0x714585`).
            let active = transitions.play(&mut player, clip.node, std::time::Duration::ZERO);
            if clip.looping {
                active.repeat();
            }
        }
    }
    commands.entity(entity).insert((
        player,
        AnimationGraphHandle(anims.graph.clone()),
        anims.clone(),
    ));
    match kind {
        EntityKind::GameObject if go_state_machine => {
            commands.entity(entity).insert((
                // Cross-fades each transition over the clip's blend-in time, from the seed.
                transitions,
                crate::go_anim::GoAnim::default(),
            ));
        }
        // A loader-idle GameObject needs no driver: the idle is its whole animation.
        EntityKind::GameObject => {}
        // A corpse gets no `AnimDriver`: its pose is one settled arm (`pose_corpses`), and the
        // unit gait selector would stand it up.
        EntityKind::Corpse => {
            commands
                .entity(entity)
                .insert(AnimationTransitions::new())
                .remove::<benilla_world::rig_anim::AnimParked>();
        }
        _ => {
            commands
                .entity(entity)
                .insert((
                    // Cross-fades over each clip's blend-in time, so a gait change eases.
                    AnimationTransitions::new(),
                    AnimDriver::default(),
                ))
                // A rebuilt rig is born live: a stale park marker would desync the LOD gate's
                // edge-triggered bookkeeping.
                .remove::<benilla_world::rig_anim::AnimParked>();
        }
    }
}

/// A rig's build result: the pose, held until the build has resolved its anchors into it
/// (`anchor_for` needs `&mut`), and the palette slot each skinned part tags.
struct RigBuild {
    pose: benilla_world::rig_anim::RigPose,
    slot: u16,
}

/// Attach a visual to each net entity without one, our own avatar included. The net bridge or the
/// player controller owns the entity's transform; only its scale is set here.
#[allow(clippy::type_complexity)]
pub(super) fn attach_entity_visuals(
    mut commands: Commands,
    pending: Query<
        (
            Entity,
            &NetEntity,
            Option<&super::Equipment>,
            Has<super::equipment::Reattached>,
            Option<&super::mount::MountChild>,
            Option<&super::mount::MountBody>,
            Has<crate::transport::TransportAnchor>,
        ),
        Without<VisualAttached>,
    >,
    // A mounted rider waits on its mount child, whose attachment-0 point is the seat.
    mount_children: super::mount::MountChildren,
    // The mount child's pose, for its seat anchor; the unit being built has none yet, so the two
    // never alias.
    mut built_poses: Query<&mut benilla_world::rig_anim::RigPose>,
    displays: Option<Res<super::ItemDisplays>>,
    mut transforms: Query<&mut Transform>,
    assets: Res<CubeAssets>,
    creatures: Option<Res<Creatures>>,
    gameobjects: Option<Res<GameObjects>>,
    // The bone-pile models, a corpse's second model source.
    bones: Res<super::corpse::BonesModels>,
    // The geoset tables; absent, every geoset shows.
    characters: Option<Res<Characters>>,
    stores: Query<&ObjectStore>,
    // The character-skin build chain, nested for Bevy's 16-param system limit.
    skin_build: (
        Option<Res<SkinSections>>,
        Option<Res<WorldAssets>>,
        ResMut<Assets<Image>>,
        ResMut<SkinComposites>,
        Res<AssetServer>,
        benilla_world::model_render::M2BatchMaterials,
        ResMut<Assets<Mesh>>,
        ResMut<merge::MergedFormsCache>,
        ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
        ResMut<benilla_world::doodad_anim::TintAnimMaterials>,
        ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    ),
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
    mut collider_epoch: ResMut<benilla_world::collision::ColliderEpoch>,
    time: Res<Time>,
) {
    let (
        sections,
        world_assets,
        mut images,
        mut skin_composites,
        asset_server,
        mut mats,
        mut meshes,
        mut merged,
        mut uv_reg,
        mut tint_reg,
        mut anim_table,
    ) = skin_build;
    let now = time.elapsed_secs();
    for (entity, net, equipment, reattached, mount_child, mount_body, anchored) in &pending {
        // A player attaches once its equipment settles, so it never flashes naked.
        if net.kind == EntityKind::Player && !equipment.is_some_and(|e| e.settled) {
            continue;
        }
        // A corpse waits for `resolve_corpse_equipment`: it is never re-dressed, so a body
        // composited early would stay naked.
        if net.kind == EntityKind::Corpse && equipment.is_none() {
            continue;
        }
        // A corpse's model follows `CORPSE_FLAG_BONES`, the creature chain or the
        // `<Race><Sex>DeathSkeleton` props (`0x5d6700`), through `corpse_model`, which the display
        // build reads too.
        let dm = match net.kind {
            EntityKind::Corpse => match super::corpse::corpse_model(net, stores.get(entity).ok()) {
                Some(super::corpse::CorpseModel::Bones(race, sex)) => bones.0.get(&(race, sex)),
                Some(super::corpse::CorpseModel::Flesh(disp)) => {
                    creatures.as_deref().and_then(|c| c.models.get(&disp))
                }
                None => None,
            },
            _ => net.display_id.and_then(|disp| match net.kind {
                EntityKind::Unit | EntityKind::Player => {
                    creatures.as_deref().and_then(|c| c.models.get(&disp))
                }
                EntityKind::GameObject => gameobjects.as_deref().and_then(|g| g.models.get(&disp)),
                _ => None,
            }),
        };
        let worn = resolve_worn_equip(net, equipment, dm);
        let equip = worn.bodyslots;
        // An empty `parts` list is either a model that draws nothing (an answer) or no model at
        // all (a gap of ours, which the cube shows).
        let named_a_model = dm.is_some_and(DisplayModel::names_a_model);
        // A loading model (`parts == None`) retries next frame. A model that built no batch is
        // still a loaded instance, as in the reference: its skeleton, attachment points, clock,
        // name anchor and emitters all come from it.
        let model = match dm {
            Some(d) => match &d.parts {
                None => continue,
                Some(parts) => named_a_model.then_some(parts.as_slice()),
            },
            None => None,
        };

        // Invisible GameObjects and trigger creatures hide by transparent or empty M2s, never by a
        // flag: the reference draws any loaded model and culls zero-alpha batches (`0x707b3a`),
        // so their `parts` are empty and `named_a_model` keeps the cube off them.
        if let Some(parts) = model {
            // A mounted unit is two skeletons: the mount is a child entity this system builds like
            // any beast, and the rider's rig roots at its attachment-0 seat. The unit waits for
            // the mount; a mount change on a standing unit is `mount::reseat_mounts`' job.
            let mount_display = match net.kind {
                EntityKind::Unit | EntityKind::Player => stores
                    .get(entity)
                    .map_or(0, |s| s.0.unit_mount_display_id()),
                _ => 0,
            };
            // The rider's rig root: the unit, or the seat anchor.
            let mut rider_root = entity;
            if mount_display != 0 {
                match super::mount::seat_or_spawn_mount(
                    &mut commands,
                    &mount_children,
                    &mut built_poses,
                    creatures.as_deref(),
                    entity,
                    mount_child.map(|&super::mount::MountChild(c)| c),
                    mount_display,
                    reattached,
                ) {
                    super::mount::Seat::Wait => continue,
                    super::mount::Seat::Frame(anchor) => rider_root = anchor,
                    // The reference leaves the body at the unit matrix (`0x60ce70`'s present-test
                    // miss).
                    super::mount::Seat::UnitMatrix => {}
                }
            } else if let Some(&super::mount::MountChild(child)) = mount_child {
                // Dismounted while pending: drop the mount it ordered.
                if let Ok(mut ec) = commands.get_entity(child) {
                    ec.despawn();
                }
                commands.entity(entity).remove::<super::mount::MountChild>();
            }
            let kind = match net.kind {
                EntityKind::GameObject => ModelKind::GameObject,
                _ => ModelKind::Creature,
            };
            let emitters = dm.map(|d| d.emitters.as_slice()).unwrap_or_default();
            let model_lights = dm.map(|d| d.lights.as_slice()).unwrap_or_default();
            // The model-local interior fold point; the classifier applies the entity scale.
            let bake_center = dm.map(|d| d.bake_center_local).unwrap_or(Vec3::ZERO);
            // One ground-shade state per object (the reference's `[obj+0xe0]`), the same 2.5/0.5
            // MCSH chase for every kind; `insert_if_new` so a rebuild keeps the ramped state.
            commands
                .entity(entity)
                .insert_if_new(benilla_world::entity_shade::GroundShade::default());
            // Held items share the body's interior verdict (the reference aliases the wearer's
            // collector into each item, `0x718960`), so they fold at the body's centre.
            commands
                .entity(entity)
                .insert(benilla_world::interior::BodyBakeCenter(bake_center));
            // The light node's attach mode (`[node+0x90]` bit 13, set from the TYPEMASK at
            // `0x613e10`/`0x670db0`, dispatched at `0x6a86d0`): a GameObject by containment at its
            // bounds centre (`0x6a8c10`), a unit by down-ray at its position (`0x6a8a20`).
            match net.kind {
                EntityKind::GameObject => {
                    commands
                        .entity(entity)
                        .insert(benilla_world::interior::ContainmentAttach);
                }
                _ => {
                    commands
                        .entity(entity)
                        .remove::<benilla_world::interior::ContainmentAttach>();
                }
            }
            let object = WorldObject {
                kind,
                label: dm.map(|d| display_label(&d.handle)).unwrap_or_default(),
                id: net.display_id.unwrap_or(0),
                detail: format!("emitters: {}", emitters.len()),
            };
            // Whether a GameObject runs the open/close state machine (the type dispatch at
            // `0x5f76cc`); read once, so the rig and the clock's driver agree.
            let go_state_machine = net.kind == EntityKind::GameObject
                && stores
                    .get(entity)
                    .is_ok_and(|s| crate::go_anim::go_animates(s.0.gameobject_type_id()));
            // Whether anything skins through the palette slot: not for a body that draws nothing.
            let skins = parts.iter().any(|p| p.skinned_mesh.is_some());
            let mut skin: Option<RigBuild> = match (net.kind, dm) {
                // A boneless model stays static: its skinned twin would index joints that do not
                // exist.
                (EntityKind::Unit | EntityKind::Player, Some(d))
                    if !d.skeleton.joints.is_empty() =>
                {
                    // Terrain conform: a flagged model's root bones sit under a node
                    // `conform_units` rotates. A mounted rider never gets one: the reference
                    // dispatches on the mount's model (`0x7106c0`) and tilts through its node.
                    let mut joints_root = rider_root;
                    if d.terrain_tilt != 0 && rider_root == entity {
                        let node = commands
                            .spawn((
                                super::conform::ConformNode {
                                    // The ground and yaw source: the unit, or a mount child's
                                    // host.
                                    unit: mount_body.map_or(entity, |mb| mb.host),
                                    mode: d.terrain_tilt,
                                },
                                Transform::default(),
                                Visibility::default(),
                            ))
                            .id();
                        commands.entity(entity).add_child(node);
                        joints_root = node;
                    }
                    setup_skinned_instance(
                        &mut commands,
                        &mut palettes,
                        entity,
                        joints_root,
                        d,
                        skins,
                    )
                }
                // A fresh corpse skins, or its Death pose would stand in bind pose; a bone pile, a
                // static prop with no sequence, takes no palette slot.
                (EntityKind::Corpse, Some(d))
                    if !d.skeleton.joints.is_empty() && d.animations.is_some() =>
                {
                    setup_skinned_instance(&mut commands, &mut palettes, entity, entity, d, skins)
                }
                // An animated GameObject skins like a creature. A door, button or chest
                // (`go_animates`) runs the `GAMEOBJECT_STATE` machine; any other (a mailbox's
                // flags, a windmill) plays the loader idle the reference arms on every M2 instance
                // (`0x70ebd0`), which `ModelAnimations::idle_clip` picks: id 0 through the playable
                // lookup, else the first clip. One whose idle holds its rest pose with no global
                // sequence (`DoodadAnimTier::Static`) keeps the static mesh.
                (EntityKind::GameObject, Some(d))
                    if !d.skeleton.joints.is_empty() && d.animations.is_some() =>
                {
                    let state_machine = go_state_machine;
                    let ambient = !matches!(
                        benilla_world::doodad_anim::classify(&d.skeleton, d.animations.as_ref()),
                        benilla_world::doodad_anim::DoodadAnimTier::Static
                    );
                    // A state GO whose sequences pose no bone gets no rig, only the clock below.
                    let poses = d
                        .animations
                        .as_ref()
                        .is_some_and(|a| a.clips.iter().any(|c| c.poses_bones));
                    ((state_machine && poses) || ambient)
                        .then(|| {
                            setup_skinned_instance(
                                &mut commands,
                                &mut palettes,
                                entity,
                                entity,
                                d,
                                skins,
                            )
                        })
                        .flatten()
                }
                _ => None,
            };
            // The sequence clock, rigged or not.
            if matches!(
                net.kind,
                EntityKind::Unit | EntityKind::Player | EntityKind::GameObject | EntityKind::Corpse
            ) {
                if let Some(anims) = dm.and_then(|d| d.animations.as_ref()) {
                    arm_sequence_clock(&mut commands, entity, anims, net.kind, go_state_machine);
                }
            }
            // The attachment points and event markers that held items and other riders hang from.
            if let (Some(_), Some(d)) = (&skin, dm) {
                // The first marker of an ident wins, as the reference's scan (`0x7130e0`) returns
                // it: character models carry six `$CSD` records.
                let mut markers = std::collections::HashMap::new();
                for m in &d.markers {
                    markers.entry(m.ident).or_insert((m.bone, m.offset));
                }
                commands.entity(entity).insert(super::BoneAttach {
                    points: d
                        .attachments
                        .iter()
                        .map(|a| (a.id, (a.bone, a.offset)))
                        .collect(),
                    markers,
                });
                // The strafe counter-twist on the SpineLow and Head key-bones; a model with neither
                // gets none.
                let nb = d.skeleton.joints.len();
                let in_range = |b: Option<u16>| b.filter(|&i| (i as usize) < nb);
                let (spine, head) = (
                    in_range(d.skeleton.spine_bone),
                    in_range(d.skeleton.head_bone),
                );
                if spine.is_some() || head.is_some() {
                    commands
                        .entity(entity)
                        .insert(crate::creature_anim::BodyTwist::new(spine, head));
                }
            }
            // A character model carries every hair, facial and body geoset, and is shared by its
            // display, so the entity's own look picks which show.
            let look = resolve_char_look(net, dm, entity, &stores);
            // The worn geoset selectors, the helm's hide-mask rows included (`0x4799a0`).
            let equip_geosets = equip_geosets(
                displays.as_deref(),
                &equip,
                worn.cloak,
                worn.helm,
                worn.tabard_preview,
            );
            let visible_geosets: Option<Vec<u16>> = look.as_ref().and_then(|l| {
                let cg = characters.as_deref()?;
                Some(cg.0.visible_geosets(
                    l.race,
                    l.sex,
                    l.hair_style,
                    l.facial_hair,
                    &equip_geosets,
                ))
            });
            let char_mats = match look.as_ref() {
                Some(l) => build_char_skin_materials(
                    l,
                    equip,
                    worn.cloak,
                    worn.emblem,
                    worn.tabard_preview,
                    displays.as_deref(),
                    sections.as_deref(),
                    world_assets.as_deref(),
                    parts,
                    &mut images,
                    &mut skin_composites.0,
                    &asset_server,
                    &mut mats,
                ),
                None => (None, None, None, (None, None)),
            };
            // Whether a part armed an appear-fade, mirrored onto the root so a held item that
            // spawns later joins the ramp.
            let mut unit_will_fade = false;
            let inst_slot = skin.as_ref().map_or(0, |rb| rb.slot);
            // The armed idle's authored CAaBox, the picker's volume for a skinned part: the bind
            // box can miss the drawn model (`DuelingFlag.m2` is modelled 9 yd up and its Stand
            // translates the root −9.124 to plant it).
            let idle_aabb = dm.and_then(|m| m.animations.as_ref()).and_then(|a| {
                let clip = a.first_seq.and_then(|i| a.clips.get(i))?;
                (clip.bounds_max.cmpgt(clip.bounds_min).all()).then(|| {
                    bevy::camera::primitives::Aabb::from_min_max(clip.bounds_min, clip.bounds_max)
                })
            });
            // The body's election bound: in a sealed WMO room the reference submits no outdoor
            // object, so the cull needs one box per body (restated onto `WorldUnit::bound`). It
            // reads `idle_clip`, not `first_seq`, which is `None` for an idle at its rest pose and
            // would exempt the body; a degenerate authored box falls back to the bind extent.
            let election_bound = dm
                .and_then(|m| m.animations.as_ref())
                .and_then(|a| a.idle_clip())
                .filter(|c| c.bounds_max.cmpgt(c.bounds_min).all())
                .map(|c| bevy::camera::primitives::Aabb::from_min_max(c.bounds_min, c.bounds_max))
                .or_else(|| union_aabb(model.unwrap_or_default().iter().filter_map(|p| p.aabb)));
            if let Some(aabb) = election_bound {
                commands.entity(entity).insert(super::ModelBound(aabb));
            }
            // A billboard card rides its live joint, so card anchors resolve here, over the spawn
            // loop's geoset predicate: a hidden variant's card bone spawns no anchor.
            let card_anchors: std::collections::HashMap<u16, Entity> = match skin.as_mut() {
                Some(rb) => parts
                    .iter()
                    .filter(|part| {
                        !visible_geosets
                            .as_ref()
                            .is_some_and(|vis| !vis.contains(&part.geoset_id))
                    })
                    .filter_map(|part| part.billboard.as_ref().map(|b| b.bone))
                    .filter_map(|bone| {
                        rb.pose
                            .anchor_for(&mut commands, entity, bone)
                            .map(|a| (bone, a))
                    })
                    .collect(),
                None => std::collections::HashMap::new(),
            };
            let dress = PartDress {
                unit: entity,
                kind,
                char_mats: &char_mats,
                object: &object,
                inst_slot,
                rigged: skin.is_some(),
                anchors: card_anchors,
                bake_center,
                idle_aabb,
                now,
                // A fresh visual fades in; a rebuild (`Reattached`: a mount transition, a display
                // swap) spawns steady.
                fade: if reattached {
                    JoinedFade::Steady
                } else {
                    JoinedFade::Pending { since: now }
                },
            };
            // The shown batches spawn as material groups (`merge`).
            let shows = |part: &super::EntityPart| {
                visible_geosets
                    .as_ref()
                    .is_none_or(|vis| vis.contains(&part.geoset_id))
            };
            let groups = merge::guard_groups(merge::group_parts(parts, shows), parts, &char_mats);
            for group in &groups {
                let forms = merged.forms(parts, group, &mut meshes);
                let part = merge::group_part(parts, group, forms);
                unit_will_fade |= spawn_group(
                    &mut commands,
                    &part,
                    group,
                    &dress,
                    &mut dress::OwnMats {
                        store: mats.materials(),
                        uv: &mut uv_reg,
                        tint: &mut tint_reg,
                        table: &mut anim_table,
                    },
                );
            }
            // A `Reattached` rebuild drops any in-flight fade, as its parts spawned steady.
            if unit_will_fade {
                commands
                    .entity(entity)
                    .insert(benilla_world::model_fade::UnitAppearFade::Pending { since: now });
            } else if reattached {
                commands
                    .entity(entity)
                    .remove::<benilla_world::model_fade::UnitAppearFade>();
            }
            // Size is `OBJECT_FIELD_SCALE_X` alone: the server already folds the DBC scale into
            // it (vmangos `Unit::GetScaleForDisplayId`, `Unit.cpp:9579`), and the reference
            // renders at the field (`0x613ef0`).
            let placement = if let Ok(mut t) = transforms.get_mut(entity) {
                t.scale = Vec3::splat(net.scale);
                *t
            } else {
                Transform::default()
            };
            // What the visual was dressed with, for `redress_player_looks` to diff.
            if let (EntityKind::Player, Some(e)) = (net.kind, equipment) {
                commands
                    .entity(entity)
                    .insert(super::equipment::AppliedEquipment(*e));
            }
            // The camera's framing pivot, model-local, before scale.
            commands.entity(entity).insert(CameraPivot {
                height_local: dm.map(|d| d.pivot_height_local).unwrap_or(0.0),
                // The pivot's drop while swimming, the reference's third preset `cam+0x124`
                // (`0x50ccf6`); 0 without a Swim sequence.
                swim_drop_local: dm.map(|d| d.swim_pivot_drop_local).unwrap_or(0.0),
            });
            // The overhead fallback for a model with no PlayerName attachment (`0x608640`).
            commands.entity(entity).insert(super::OverheadFallback(
                dm.map(|d| d.bbox_z_local).unwrap_or(0.0),
            ));
            // The chat bubble's height: the Stand box's Z, not the posed PlayerName attachment.
            commands.entity(entity).insert(super::StandBoxHeight(
                dm.map(|d| d.stand_box_z_local).unwrap_or(0.0),
            ));
            // The selection ring's model-local radius, scaled by the unit's scale.
            commands.entity(entity).insert(SelectionRadius(
                dm.map(|d| d.ground_radius_local).unwrap_or(0.0),
            ));
            // Emitters ride their host bone's anchor, origin rebased into the bone frame
            // (`position − pivot`), as a doodad's do; an unrigged model's follow the entity.
            {
                for em in emitters {
                    let owner = skin
                        .as_mut()
                        .and_then(|rb| rb.pose.anchor_for(&mut commands, entity, em.def.bone))
                        .map_or((entity, [0.0; 3]), |j| (j, em.bone_pivot));
                    particles::spawn_emitter(
                        &mut commands,
                        em,
                        placement,
                        particles::EmitterFrames {
                            owner: Some(owner),
                            // The cloud sorts at the unit; bones compose births only, and a
                            // world-mode particle is frozen at birth.
                            anchor: Some(entity),
                            // The unit's model is the emitter's model: free the pool with it.
                            on_owner_loss: particles::OwnerLoss::Free,
                            // The unit's render alpha multiplies its clouds.
                            alpha: Some(entity),
                            // The entity is the light node, filled whether or not a batch reads
                            // it; a lit emitter consumes it as a lit batch does.
                            light_node: Some(entity),
                        },
                        // Rate and enable read the instance's playing sequence.
                        particles::EmitClock::Host(entity),
                    );
                }
            }
            // The model's point lights ride their bones, as the reference re-registers each at its
            // live bone every frame. Their anchors resolve first: the call holds `commands`.
            let light_anchors: std::collections::HashMap<i16, Entity> = model_lights
                .iter()
                .filter(|l| l.def.casts())
                .filter_map(|l| {
                    let bone = u16::try_from(l.def.bone).ok()?;
                    let rb = skin.as_mut()?;
                    rb.pose
                        .anchor_for(&mut commands, entity, bone)
                        .map(|a| (l.def.bone, a))
                })
                .collect();
            super::spawn_carried_lights(&mut commands, model_lights, entity, |bone| {
                light_anchors.get(&bone).copied()
            });
            // Ribbon trails, on the same host-bone ride; each despawns with its owner.
            {
                for rb in dm.map(|d| d.ribbons.as_slice()).unwrap_or_default() {
                    let (owner, use_pivot) = skin
                        .as_mut()
                        .and_then(|build| build.pose.anchor_for(&mut commands, entity, rb.def.bone))
                        .map_or((entity, false), |j| (j, true));
                    // The `+0xc0` enable gate reads the instance's live sequence: a GameObject
                    // changes state under its own trails.
                    benilla_world::ribbons::spawn_ribbon(
                        &mut commands,
                        rb,
                        owner,
                        use_pivot,
                        placement.scale.max_element(),
                        benilla_world::ribbons::RibbonSeq::Host(entity),
                        // The unit's own render alpha gates its trail.
                        Some(entity),
                        // No fade sphere: server visibility bounds a unit's population.
                        None,
                    );
                }
            }
            // The pose lands last, with every anchor the build resolved into it.
            if let Some(rb) = skin {
                commands.entity(entity).insert(rb.pose);
            }
            // A GameObject with a hull collides; an anchored transport stays Kinematic, as its arm
            // labelled it, since its body moves every frame.
            if matches!(net.kind, EntityKind::GameObject) {
                if let Some(col) = dm.and_then(|d| d.collider.clone()) {
                    let body = if anchored {
                        RigidBody::Kinematic
                    } else {
                        RigidBody::Static
                    };
                    commands.entity(entity).insert((body, col));
                    // This lane bypasses the streamer's attach queue, so it stamps the collider
                    // set itself.
                    collider_epoch.bump();
                }
            }
        } else {
            // A debug cube (a player cyan, an NPC red, lifted onto the ground) for a display that
            // named no model, a gap of ours. One that named a model draws what it built, maybe
            // nothing: every type reaches the same batch loop through `0x613e10`, and its trip
            // count is the view's texUnit count (`0x707a72`).
            let fallback = match net.kind {
                _ if named_a_model => None,
                EntityKind::Player => {
                    Some((assets.player_mat.clone(), assets.player_mesh.clone(), 1.0))
                }
                EntityKind::Unit => Some((assets.npc_mat.clone(), assets.mesh.clone(), 2.0)),
                EntityKind::GameObject => {
                    debug!(
                        "gameobject (display {:?}) has no usable model — not rendering",
                        net.display_id
                    );
                    None
                }
                // A model-less corpse draws nothing, as the reference's null-row legs do
                // (`0x5d6700` returns 0); a DynamicObject's look is its spell's area effect.
                EntityKind::Corpse => {
                    debug!(
                        "corpse (display {:?}) has no usable model — not rendering",
                        net.display_id
                    );
                    None
                }
                EntityKind::DynamicObject | EntityKind::Other => None,
            };
            if let Some((material, mesh, lift)) = fallback {
                // Pickable, so a model-less unit can still be targeted.
                let object = WorldObject {
                    kind: ModelKind::Creature,
                    label: format!("{:?} (no model)", net.kind),
                    id: net.display_id.unwrap_or(0),
                    detail: String::new(),
                };
                commands.entity(entity).with_children(|parent| {
                    parent.spawn((
                        Mesh3d(mesh),
                        MeshMaterial3d(material),
                        Transform::from_translation(Vec3::Y * lift),
                        object,
                        benilla_world::interact::CreaturePickPart,
                        // No `RenderSubmesh`: the picker casts against the cube's box.
                        benilla_world::interact::PickBox,
                        // Counted by `WOW_UNIT_VISUALS`.
                        super::FallbackCube,
                    ));
                });
            }
        }
        // The mount the visual was built on, for `reseat_mounts` to diff; stamped on a cube too,
        // so a model-less unit never churns.
        if matches!(net.kind, EntityKind::Unit | EntityKind::Player) {
            let applied = stores
                .get(entity)
                .map_or(0, |s| s.0.unit_mount_display_id());
            commands
                .entity(entity)
                .insert(super::mount::AppliedMount(applied));
        }
        commands
            .entity(entity)
            .insert(VisualAttached)
            // The display the visual was built with, for `refresh_live_display` to diff.
            .insert(super::live_display::AppliedDisplay(net.display_id))
            .remove::<super::equipment::Reattached>();
    }
}

/// The union of a model's part boxes, its bind-pose extent: the election bound when the idle
/// authors no CAaBox. Over-large only admits, never hides.
fn union_aabb(
    boxes: impl Iterator<Item = bevy::camera::primitives::Aabb>,
) -> Option<bevy::camera::primitives::Aabb> {
    let mut boxes = boxes;
    let first = boxes.next()?;
    let (mut min, mut max) = (first.min(), first.max());
    for b in boxes {
        min = min.min(b.min());
        max = max.max(b.max());
    }
    Some(bevy::camera::primitives::Aabb::from_min_max(
        Vec3::from(min),
        Vec3::from(max),
    ))
}

/// A display model's asset path as an inspector label, empty without one.
fn display_label(handle: &ModelHandle) -> String {
    let path = match handle {
        ModelHandle::M2(h) => h.path(),
        ModelHandle::Wmo(h) => h.path(),
        ModelHandle::None => None,
    };
    path.map(|p| p.path().to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_assets::{ModelJoint, ModelSkeleton};
    use bevy::ecs::world::CommandQueue;
    use bevy::mesh::skinning::SkinnedMeshInverseBindposes;

    /// The smallest rig `setup_skinned_instance` builds.
    fn one_bone(ibps: &mut Assets<SkinnedMeshInverseBindposes>) -> DisplayModel {
        DisplayModel {
            skeleton: ModelSkeleton {
                joints: vec![ModelJoint {
                    parent: -1,
                    local_translation: Vec3::ZERO,
                    billboard: None,
                    parent_arm: None,
                }],
                spine_bone: None,
                head_bone: None,
            },
            inverse_bindposes: Some(
                ibps.add(SkinnedMeshInverseBindposes::from(vec![Mat4::IDENTITY])),
            ),
            ..super::super::display::empty_shell()
        }
    }

    /// A body with nothing to skin takes no `RigStarved` either, or `heal_rig_starved` would
    /// rebuild it for ever.
    #[test]
    fn a_body_that_skins_nothing_takes_the_rig_without_a_slot() {
        use benilla_world::rig_palette::{RigPalettes, RigSkin, RigStarved};

        let mut app = App::new();
        app.init_resource::<RigPalettes>()
            .init_resource::<Assets<SkinnedMeshInverseBindposes>>();
        let mut ibps = app
            .world_mut()
            .remove_resource::<Assets<SkinnedMeshInverseBindposes>>()
            .unwrap();
        let d = one_bone(&mut ibps);

        for (skins, want_slot) in [(false, false), (true, true)] {
            let mut world = std::mem::take(app.world_mut());
            let entity = world.spawn_empty().id();
            let mut palettes = world.remove_resource::<RigPalettes>().unwrap();
            let build = {
                let mut queue = CommandQueue::default();
                let mut commands = Commands::new(&mut queue, &world);
                let build =
                    setup_skinned_instance(&mut commands, &mut palettes, entity, entity, &d, skins);
                queue.apply(&mut world);
                build
            };
            world.insert_resource(palettes);
            assert!(
                build.is_some(),
                "the pose is built either way (skins={skins})"
            );
            let e = world.entity(entity);
            assert_eq!(
                e.contains::<RigSkin>(),
                want_slot,
                "skins={skins} ⇒ palette slot {want_slot}"
            );
            assert!(
                !e.contains::<RigStarved>(),
                "skins={skins}: a table with room never starves — and a body that skins nothing \
                 must never ask to be healed"
            );
            *app.world_mut() = world;
        }
    }
}
