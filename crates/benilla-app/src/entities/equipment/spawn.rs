//! Equipment attach: each item model under its attachment's joint, with everything riding it.

use bevy::mesh::MeshTag;
use bevy::prelude::*;

use benilla_world::billboard::BillboardCard;
use benilla_world::interior::part_interior_lit;
use benilla_world::model_fade::{
    join_unit_appear_fade, FadeSet, JoinedFade, PartFade, UnitAppearFade,
};
use benilla_world::model_render::{ModelKind, ModelPart};
use benilla_world::vis_chain::VisChainOnly;

use super::super::{item_glow::ItemGlow, spawn_carried_lights};
use super::{
    attach_id, BoneAttach, HeldAttached, HeldItems, HeldSlot, ItemDisplays, ATTACH_SLOTS, NO_GLOW,
};

/// The materials one item batch fades through.
fn item_fade_set(part: &super::super::EntityPart) -> FadeSet<'_> {
    FadeSet {
        steady: &part.material,
        blend: part.fade_blend.as_ref(),
        bake_blend: part.material_interior_bake_blend.as_ref(),
        zfill: part.zfill.as_ref(),
    }
}

/// What one slot's spawn needs from its wearer, read once per unit.
struct WearerCtx<'a> {
    /// The unit wearing the item, whose tint, light collector and fade it inherits.
    wearer: Entity,
    bones: &'a BoneAttach,
    /// The unit's appear-fade clock, for a part spawning mid-ramp.
    joined: JoinedFade,
    now: f32,
    /// The wearer's rig-palette slot, pre-shifted into `MeshTag` bits.
    rig_slot: u16,
    /// The wearer's body bake centre, the interior classifier's fold reference.
    body_center: Option<Vec3>,
    /// The unit's wire scale, which its held effects' draw-order rung is measured in.
    scale: f32,
}

/// Everything under an item root that caches where on the body it sits, re-seated on a move.
#[derive(bevy::ecs::system::SystemParam)]
pub(in crate::entities) struct SeatWriters<'w, 's> {
    children: Query<'w, 's, &'static Children>,
    riders: Query<'w, 's, &'static mut crate::portrait::PortraitRider>,
    cards: Query<'w, 's, &'static mut crate::portrait::PortraitBillboard>,
    effects: Query<'w, 's, &'static mut crate::portrait::PortraitEffects>,
    glows: Query<'w, 's, &'static mut ItemGlow>,
}

impl SeatWriters<'_, '_> {
    /// Move every cached seat under `root` to `bone`, shifting each offset by `delta` (a mirror's
    /// offset also holds a model-local term, a card's pivot, that stays) and setting `attach` for
    /// the widgets' attach reset. Recursive: glow instances hang two levels down.
    fn reseat(&mut self, root: Entity, bone: u16, attach: u16, delta: Vec3) {
        let mut stack = vec![root];
        while let Some(e) = stack.pop() {
            if let Ok(mut r) = self.riders.get_mut(e) {
                r.bone = bone;
                r.offset += delta;
                r.attach = Some(attach);
            }
            if let Ok(mut c) = self.cards.get_mut(e) {
                c.bone = bone;
                c.attach = Some(attach);
                c.seat = match c.seat {
                    crate::portrait::PortraitSeat::Body => crate::portrait::PortraitSeat::Body,
                    crate::portrait::PortraitSeat::Rider(at) => {
                        crate::portrait::PortraitSeat::Rider(at + delta)
                    }
                };
            }
            if let Ok(mut f) = self.effects.get_mut(e) {
                f.bone = bone;
                f.offset += delta;
                f.attach = Some(attach);
            }
            if let Ok(mut g) = self.glows.get_mut(e) {
                g.bone = bone;
                g.offset += delta;
                g.attach = attach;
            }
            if let Ok(kids) = self.children.get(e) {
                stack.extend(kids.iter());
            }
        }
    }
}

/// Spawn each changed slot's item model under its attachment's joint once the model has built.
/// The diff is per slot, and a new attach point alone moves the root: the reference's sheath
/// paths touch only the weapon and quiver ids (`0x611770`) and stow a melee weapon by re-parenting
/// its model (`0x60b590`, `0x712f70`), so the model and everything riding it survive.
#[allow(clippy::type_complexity)]
pub(in crate::entities) fn attach_held_items(
    mut commands: Commands,
    mut units: Query<(
        &HeldItems,
        &BoneAttach,
        Option<&mut HeldAttached>,
        Entity,
        Option<&UnitAppearFade>,
        Option<&benilla_world::interior::BodyBakeCenter>,
        Option<&Transform>,
        // The wearer's slot, which a slotless item's parts carry so the body tint reaches them;
        // an attached model inherits its parent's computed colours (`0x714000`).
        Option<&benilla_world::rig_palette::RigSkin>,
        // The attach joint spawns on demand from the live pose, never the rest pose.
        Option<&mut benilla_world::rig_anim::RigPose>,
    )>,
    held: Option<Res<ItemDisplays>>,
    time: Res<Time>,
    mut seats: SeatWriters,
    // A move keeps the item alive but changes the bone its palette frame is composed from.
    mut movers: Query<&mut benilla_world::rig_rider::RigRider>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
) {
    let Some(held) = held else {
        return;
    };
    let now = time.elapsed_secs();
    for (items, bones, attached, entity, unit_fade, body_center, unit_tf, skin, mut pose) in
        &mut units
    {
        // The reference fades a unit and its attachments as one, so a late part joins the ramp.
        let ctx = WearerCtx {
            wearer: entity,
            bones,
            joined: join_unit_appear_fade(unit_fade.copied()),
            now,
            rig_slot: skin.map_or(0, |rb| rb.slot),
            body_center: body_center.map(|c| c.0),
            scale: unit_tf.map_or(1.0, |t| t.scale.max_element()),
        };
        // A slot whose model has not built stays unapplied, and attaches once it has.
        let mut next = items.clone();
        for slot in next.slots.iter_mut() {
            let ready = slot.is_some_and(|hs| {
                held.models
                    .get(&(hs.display, hs.kind))
                    .and_then(|dm| dm.parts.as_ref())
                    .is_some()
            });
            if !ready {
                *slot = None;
            }
        }
        if attached.as_ref().is_some_and(|a| a.applied == next) {
            continue;
        }
        let (applied, mut roots) = attached.as_ref().map_or_else(
            || (HeldItems::default(), [None; ATTACH_SLOTS]),
            |a| (a.applied.clone(), a.spawned),
        );
        for (slot_idx, (root, (was, wants))) in roots
            .iter_mut()
            .zip(
                applied
                    .slots
                    .iter()
                    .copied()
                    .zip(next.slots.iter().copied()),
            )
            .enumerate()
        {
            if was == wants {
                continue;
            }
            // The move: same item, new attach point; everything riding the root comes along.
            if let (Some(w), Some(n), Some(root)) = (was, wants, *root) {
                if w.same_item(&n) {
                    if let Some((&(bone, offset), &(_, old))) = ctx
                        .bones
                        .points
                        .get(&n.attach)
                        .zip(ctx.bones.points.get(&w.attach))
                    {
                        if let Some(joint) = pose
                            .as_mut()
                            .and_then(|p| p.anchor_for(&mut commands, entity, bone))
                        {
                            commands.entity(joint).add_child(root);
                            commands
                                .entity(root)
                                .insert(Transform::from_translation(offset));
                            // The rider's frame is composed from (bone, offset): carry it too.
                            if let Ok(mut rider) = movers.get_mut(root) {
                                rider.bone = bone;
                                rider.local = offset;
                            }
                            seats.reseat(root, bone, n.attach, offset - old);
                            debug!(
                                "held move: unit {entity} display {} → attach {} (bone {bone})",
                                n.display, n.attach
                            );
                            continue;
                        }
                    }
                }
            }
            // A different item, or none: rebuild.
            if let Some(old) = root.take() {
                commands.entity(old).despawn();
            }
            *root = wants.and_then(|hs| {
                spawn_slot(
                    &mut commands,
                    &mut palettes,
                    &ctx,
                    pose.as_deref_mut(),
                    &held,
                    slot_idx,
                    &hs,
                )
            });
        }
        let applied = HeldAttached {
            applied: next,
            spawned: roots,
        };
        match attached {
            Some(mut a) => *a = applied,
            None => {
                commands.entity(entity).insert(applied);
            }
        }
    }
}

/// Spawn one slot's item model and everything riding it, if its parts and attach point exist.
fn spawn_slot(
    commands: &mut Commands,
    palettes: &mut benilla_world::rig_palette::RigPalettes,
    ctx: &WearerCtx,
    pose: Option<&mut benilla_world::rig_anim::RigPose>,
    held: &ItemDisplays,
    slot_idx: usize,
    hs: &HeldSlot,
) -> Option<Entity> {
    let entity = ctx.wearer;
    let (bones, joined, now) = (ctx.bones, ctx.joined, ctx.now);
    let dm = held.models.get(&(hs.display, hs.kind))?;
    let parts = dm.parts.as_ref()?;
    let &(bone, offset) = bones.points.get(&hs.attach)?;
    let joint = pose?.anchor_for(commands, entity, bone)?;
    let root = commands
        .spawn((
            Transform::from_translation(offset),
            Visibility::default(),
            // Chained to the wearer (`0x712f70` sets `[model+0x1cc]`): the wearer's render alpha
            // multiplies everything under this root, glow instances included.
            benilla_world::model_fade::ParentModel(entity),
        ))
        .vis_chain_only()
        .id();
    commands.entity(joint).add_child(root);
    // The glow hangs off the item's own attachment points, so it lives and dies with this root.
    if hs.visual != NO_GLOW {
        commands
            .entity(root)
            .insert(super::super::item_glow::ItemGlow {
                display: hs.display,
                kind: hs.kind,
                visual: hs.visual,
                // The item's seat, for the glow's booth mirrors, and its attach id, so the
                // widgets' attach reset takes the glow with the item.
                bone,
                offset,
                attach: hs.attach,
            });
    }
    // The bowstring, for a drawn bow only, the ranged slot in the left hand: `0x611e10` registers
    // `0x611ff0` for subclass Bow. The `$WTT`/`$WTB` markers are not the gate, as melee weapons
    // author them too for the swing trail (`0x6c67f0`).
    if slot_idx == 2 && hs.attach == attach_id::HAND_LEFT {
        if let Some([top, bottom]) = dm.string_anchors {
            commands.entity(root).insert(crate::bowstring::Bowstring {
                owner: entity,
                // Bone and offset: the prop flexes, so the tips move.
                top,
                bottom,
            });
        }
    }
    // The swing trail's `WTOBJECT`, one per weapon hand (`0x608d60`) and freed with the model,
    // for a model with both markers (`0x6c67f0`'s `0x7130e0` presence gate).
    if slot_idx <= 1 {
        if let Some([top, bottom]) = dm.string_anchors {
            commands
                .entity(root)
                .insert(crate::weapon_trail::WeaponTrail::new(top.1, bottom.1));
        }
    }
    // The fishing line's near anchor (`0x61f780`). The reference gates on class 2 subclass 20 and
    // `$CCH`; only one weapon model in the data authors `$CCH`, so the marker alone is equivalent.
    if slot_idx == 0 {
        if let Some(tip) = dm.cch_marker {
            commands
                .entity(root)
                .insert(crate::fishing_line::FishingPoleTip { owner: entity, tip });
        }
    }
    // The ranged prop (`[CGUnit+0xd24]`) animates off the body's `$BWP`/`$BWR` keys: BowPull (160)
    // on the pull, then Stand or BowRelease (161), a firearm's muzzle blast (`crate::ranged_flex`).
    // So it gets a pose, a player and bone anchors; only bows, crossbows and firearms author them.
    let flexes = slot_idx == 2
        && !dm.skeleton.joints.is_empty()
        && dm.animations.as_ref().is_some_and(|a| {
            a.owns(crate::ranged_flex::BOW_PULL) || a.owns(crate::ranged_flex::BOW_RELEASE)
        });
    // The item rig, for geometry welded to a billboard bone (an R14 pauldron's spikes): the
    // reference billboards an attached model like a standalone one (`0x718657`..`0x71876f`),
    // blending the bone's camera-replaced row per vertex (`0x71a460`). All seven such models are
    // keyless, so no other joint moves.
    let item_rig = dm
        .inverse_bindposes
        .as_ref()
        .filter(|_| dm.welds_billboard() && !dm.skeleton.joints.is_empty())
        .and_then(|ibp| {
            let joints =
                benilla_world::rig_palette::spawn_joints(commands, root, root, &dm.skeleton);
            if let Some(bb) =
                benilla_world::billboard::BillboardJointRig::new(&dm.skeleton, &joints, root)
            {
                commands.entity(root).insert(bb);
            }
            // A full palette table gives no rig: the static mesh and the wearer's slot instead.
            let rig = benilla_world::rig_palette::RigSkin::allocate(palettes, joints, ibp.clone())?;
            let slot = rig.slot;
            commands.entity(root).insert(rig);
            debug!(
                "item rig: display {} welds a billboard bone → palette slot {slot} ({} bones)",
                hs.display,
                dm.skeleton.joints.len()
            );
            Some(slot)
        });
    // The rider: an item at bind pose draws through one palette frame composed in the wearer's rig
    // frame, never an absolute f32 world matrix, whose ~9500 yd sums fall on a 0.98 mm grid and
    // jitter. A boneless display, or a full palette, keeps the static mesh.
    let rider = match item_rig {
        Some(_) => None,
        // Only when some drawn part has a skinned twin to index the slot.
        None => (!dm.skeleton.joints.is_empty()
            && parts
                .iter()
                .any(|p| p.billboard.is_none() && p.skinned_mesh.is_some()))
        .then(|| {
            benilla_world::rig_palette::RigSkin::allocate_bones(
                palettes,
                dm.skeleton.joints.len() as u32,
                // A resting rider's rows are the placement alone; a flexing prop's are
                // `F × model[b] × ibp[b]`.
                if flexes {
                    dm.inverse_bindposes.clone().unwrap_or_default()
                } else {
                    Handle::default()
                },
            )
        })
        .flatten()
        .map(|skin| {
            let slot = skin.slot;
            commands.entity(root).insert((
                skin,
                benilla_world::rig_rider::RigRider {
                    host: entity,
                    bone,
                    local: offset,
                    slot,
                },
            ));
            slot
        }),
    };
    // The flexing prop's pose and clock, armed as the reference arms every M2 at load (animation 0
    // through its `playableAnimationLookup`). Its palette rows stay the rider's, composed in the
    // wearer's frame, not from its own `GlobalTransform`.
    let mut prop_pose = (flexes && rider.is_some()).then(|| {
        let anims = dm.animations.as_ref().expect("flexes ⇒ animations");
        let mut player = AnimationPlayer::default();
        if let Some(idle) = anims.idle_clip() {
            let active = player.play(idle.node);
            if idle.looping {
                active.repeat();
            }
        }
        commands.entity(root).insert((
            player,
            AnimationGraphHandle(anims.graph.clone()),
            anims.clone(),
            crate::ranged_flex::RangedProp { owner: entity },
        ));
        benilla_world::rig_anim::RigPose::new(root, &dm.skeleton)
    });
    // The slot each part's `MeshTag` carries: the item's own when it has one, as the vertex stage
    // indexes the palette by it and the tint arrives up the `ParentModel` chain (`0x714000`),
    // else the wearer's, which carries the body's tint.
    let rig_slot = item_rig.or(rider).unwrap_or(ctx.rig_slot);
    // Billboard batches spawn as world-root cards below; as children they would sit at the grip.
    let mut billboard_parts = Vec::new();
    commands.entity(root).with_children(|parent| {
        for part in parts {
            if let Some(info) = &part.billboard {
                billboard_parts.push((info.clone(), part));
                continue;
            }
            // A part joining the unit's fade opens at the ramp's current alpha: it never flashes.
            let set = item_fade_set(part);
            let effective = PartFade::resolve(joined, &set);
            let (init_mat, tag_alpha) = effective.seed(&set, now);
            // An item with a palette slot draws every part's skinned twin, a slotless one `mesh`.
            let skinned = item_rig.or(rider).and(part.skinned_mesh.as_ref());
            let mut child = parent.spawn((
                Mesh3d(skinned.unwrap_or(&part.mesh).clone()),
                MeshMaterial3d(init_mat),
                Transform::default(),
                ModelPart {
                    kind: ModelKind::Creature,
                    blend: part.blend,
                },
                // The picker's triangles; the render meshes are `RENDER_WORLD`-only.
                benilla_world::interact::PickMesh(part.geometry.clone()),
                // The booth's mirror: the static mesh and steady material at the bone's bind pose.
                crate::portrait::PortraitRider {
                    static_mesh: part.mesh.clone(),
                    material: part.material.clone(),
                    bone,
                    offset,
                    attach: Some(hs.attach),
                },
            ));
            if skinned.is_some() {
                // The palette replaces the part's world matrix, so its `Aabb` is not where it
                // draws; and the reference elects a scene object once, at the body root.
                child.insert((
                    benilla_world::rig_palette::RigPart(root),
                    bevy::camera::visibility::NoFrustumCulling,
                ));
            }
            // Lit as its wearer, whose light collector an equipped item aliases
            // (`[item+0x3b8] = [wearer+0x3b8]`, `0x718960`). Every part gets a tag, interior
            // variant or not, so the tint and the ground shade reach it.
            child.insert(MeshTag(benilla_world::mesh_tag::spawn_tag(
                rig_slot, tag_alpha,
            )));
            // The build-time bound, as the render mesh is `RENDER_WORLD`-only; none when skinned.
            if let (None, Some(aabb)) = (skinned, part.aabb) {
                child.insert(aabb);
            }
            if let Some(lit) = part_interior_lit(
                &part.material,
                part.material_interior.as_ref(),
                part.material_interior_bake.as_ref(),
                ctx.body_center.unwrap_or(dm.bake_center_local),
                entity,
            ) {
                child.insert(lit);
            }
            // The authored alpha, `colourAlpha × weight` (`0x707680`), pinned to the file's first
            // sequence and composed into the tag in the unit lane's order.
            if let Some(anim) = &part.alpha_anim {
                // A flexing prop's alpha follows its own player.
                child.insert(if flexes {
                    benilla_world::doodad_anim::MatAnim::following(anim.clone(), root)
                } else {
                    benilla_world::doodad_anim::MatAnim::resting(anim.clone())
                });
            }
            if part.uv_loops().animates() {
                child.insert(benilla_world::doodad_anim::AnimMatPart);
            }
            effective.dress(&mut child, &set);
        }
    });
    // The billboard cards: world-root entities following `root`, which they despawn with.
    for (info, part) in billboard_parts {
        // The booths mirror only the unit's descendants, so the card's mirror sits under `root`,
        // seated at the attach point plus the batch's own pivot.
        commands.entity(root).with_child((
            Transform::default(),
            Visibility::default(),
            crate::portrait::PortraitBillboard {
                mesh: part.mesh.clone(),
                material: part.material.clone(),
                bone,
                seat: crate::portrait::PortraitSeat::Rider(offset + info.pivot),
                kind: info.kind,
                attach: Some(hs.attach),
            },
        ));
        // A card joins the wearer's appear-fade like its mesh siblings.
        let set = item_fade_set(part);
        let effective = PartFade::resolve(joined, &set);
        let (init_mat, tag_alpha) = effective.seed(&set, now);
        let mut card = commands.spawn((
            Mesh3d(part.mesh.clone()),
            MeshMaterial3d(init_mat),
            Transform::default(),
            ModelPart {
                kind: ModelKind::Creature,
                blend: part.blend,
            },
            // The picker's triangles, pivot-centred by the caster like the bake.
            benilla_world::interact::PickMesh(part.geometry.clone()),
            BillboardCard::following(&info, root),
        ));
        if let Some(aabb) = part.aabb {
            card.insert(aabb);
        }
        // Tagged and lit as its mesh siblings, anchored at the wearer.
        card.insert(MeshTag(benilla_world::mesh_tag::spawn_tag(
            rig_slot, tag_alpha,
        )));
        if let Some(lit) = part_interior_lit(
            &part.material,
            part.material_interior.as_ref(),
            part.material_interior_bake.as_ref(),
            ctx.body_center.unwrap_or(dm.bake_center_local),
            entity,
        ) {
            card.insert(lit);
        }
        // The card's authored alpha, pinned as the mesh parts' is.
        if let Some(anim) = &part.alpha_anim {
            card.insert(benilla_world::doodad_anim::MatAnim::resting(anim.clone()));
        }
        if part.uv_loops().animates() {
            card.insert(benilla_world::doodad_anim::AnimMatPart);
        }
        effective.dress(&mut card, &set);
    }
    // The item's emitters: free entities following `root` at rest pose, despawned with it. The
    // spawn transform does not place them: its translation seeds the flicker RNG per item and its
    // scale is the wearer's, which the effects' draw-order rung is measured in.
    let spawn_tf = Transform::from_translation(Vec3::splat(root.to_bits() as f32))
        .with_scale(Vec3::splat(ctx.scale));
    // The booths' mirror of those emitters, which are not unit descendants.
    if !dm.emitters.is_empty() {
        commands
            .entity(root)
            .insert(crate::portrait::PortraitEffects {
                bone,
                offset,
                attach: Some(hs.attach),
                emitters: dm.emitters.clone(),
            });
    }
    for em in &dm.emitters {
        // A billboard bone in the chain puts the reference's emitter origin at
        // `pivot + camBasis·(position − pivot)`, so the emitter follows a mesh-less billboard frame
        // at rest pose. A flexing prop's emitter rides its bone's anchor, rebased by `bone_pivot`:
        // the firearm muzzle bone turns 90° in BowRelease.
        let posed = prop_pose
            .as_mut()
            .filter(|_| em.billboard.is_none())
            .and_then(|p| p.anchor_for(commands, root, em.def.bone))
            .map(|anchor| (anchor, em.bone_pivot));
        let owner = match em.billboard {
            Some(benilla_assets::EmitterBillboard { kind, pivot, .. }) => {
                let frame = commands
                    .spawn(BillboardCard::frame_following(
                        kind,
                        benilla_assets::coords::wow_to_bevy(pivot),
                        root,
                    ))
                    .id();
                let d = (0..3)
                    .map(|c| (em.def.position[c] - pivot[c]).powi(2))
                    .sum::<f32>()
                    .sqrt();
                debug!(
                    "item fx: display {} bone {} rides a {kind:?} billboard frame \
                             (pivot {pivot:?}, chain offset {d:.3} yd)",
                    hs.display, em.def.bone
                );
                (frame, pivot)
            }
            None => (root, [0.0; 3]),
        };
        benilla_world::particles::spawn_emitter(
            commands,
            em,
            spawn_tf,
            benilla_world::particles::EmitterFrames {
                owner: Some(posed.unwrap_or(owner)),
                // The cloud anchors at the model; the bone composes births only.
                anchor: Some(root),
                // The reference frees a model's emitters with the model: no cloud is left behind.
                on_owner_loss: benilla_world::particles::OwnerLoss::Free,
                // The item root, whose chain is an attached model's alpha (`0x714000`).
                alpha: Some(root),
                // The wearer's light node, aliased into each attached model (`0x718960`).
                light_node: Some(entity),
            },
            // A resting item's emitters run its loader-idle sequence; a flexing prop's the one it
            // plays, since a gun's blast is keyed only in BowRelease.
            if flexes {
                benilla_world::particles::EmitClock::Host(root)
            } else {
                benilla_world::particles::EmitClock::Pinned
            },
        );
    }
    // The item's own M2 point lights (a held torch's glow), riding `root` like the emitters.
    spawn_carried_lights(commands, &dm.lights, root, |_| None);
    // The ribbons ride the item root in Stand, where a thrown weapon's flight trail is keyed dark.
    for rb in &dm.ribbons {
        benilla_world::ribbons::spawn_ribbon(
            commands,
            rb,
            root,
            false,
            ctx.scale,
            // Stand, the sequence a worn item rests in.
            benilla_world::ribbons::RibbonSeq::Fixed(0),
            // The item root, chained to the wearer, for its alpha.
            Some(root),
            // No fade sphere: a carried item follows its wearer's residency.
            None,
        );
    }
    // Last, so every `anchor_for` above is already registered in it.
    if let Some(pose) = prop_pose {
        debug!(
            "ranged flex: unit {entity} display {} → prop clock ({} bones, {} anchors)",
            hs.display,
            pose.locals.len(),
            pose.anchors.len()
        );
        commands.entity(root).insert(pose);
    }
    debug!(
        "held attach: unit {entity} display {} → attach {} (bone {bone}, {} parts)",
        hs.display,
        hs.attach,
        parts.len()
    );
    Some(root)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::super::super::display::{empty_display, EntityPart};
    use super::super::{HeldSlot, ItemModelKind};
    use super::*;

    /// One synthetic item part, with or without an interior material variant.
    fn part(interior: bool) -> EntityPart {
        EntityPart {
            mesh: Handle::default(),
            geometry: std::sync::Arc::new(benilla_formats::RenderSubmesh::default()),
            aabb: None,
            skinned_mesh: None,
            welded_billboard: false,
            material: Handle::default(),
            material_interior: interior.then(Handle::default),
            material_interior_bake: None,
            material_interior_bake_blend: None,
            fade_blend: None,
            zfill: None,
            blend: benilla_formats::ModelBlend::Opaque,
            additive: false,
            two_sided: false,
            geoset_id: 0,
            char_slot: None,
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

    /// A mesh handle distinct from the static form's `Handle::default()`.
    fn skinned_handle() -> Handle<Mesh> {
        Handle::Uuid(
            bevy::asset::uuid::Uuid::from_u128(0x5c1_11ed),
            std::marker::PhantomData,
        )
    }

    /// What the shoulder harness reports: the parts' tags, the wearer's and the item's palette
    /// slots, the mesh drawn, the item root's rider and the joint rig count.
    struct Attached {
        tags: Vec<u32>,
        wearer_slot: u16,
        item_rig: Option<u16>,
        mesh: Option<Handle<Mesh>>,
        rider: Option<benilla_world::rig_rider::RigRider>,
        joints: usize,
    }

    fn attach_a_shoulder(welded: bool) -> Attached {
        const KIND: ItemModelKind = ItemModelKind::ShoulderLeft;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<benilla_world::rig_palette::RigPalettes>();
        let mut displays = ItemDisplays::icons_for_tests(
            benilla_formats::ItemDisplayCatalog::from_displays(HashMap::new()),
        );
        let mut dm = empty_display();
        let mut p = part(false);
        p.welded_billboard = welded;
        p.skinned_mesh = Some(skinned_handle());
        dm.parts = Some(vec![p]);
        // A root and one spherical-billboard spike, as `LShoulder_Plate_PVPAlliance_A_01` has.
        dm.skeleton = benilla_assets::ModelSkeleton {
            joints: vec![
                benilla_assets::ModelJoint {
                    parent: -1,
                    local_translation: Vec3::ZERO,
                    billboard: None,
                    parent_arm: None,
                },
                benilla_assets::ModelJoint {
                    parent: 0,
                    local_translation: Vec3::Y,
                    billboard: Some(benilla_formats::BillboardKind::Spherical),
                    parent_arm: None,
                },
            ],
            spine_bone: None,
            head_bone: None,
        };
        dm.inverse_bindposes = Some(Handle::default());
        displays.models.insert((7, KIND), dm);
        app.insert_resource(displays);

        let bones = BoneAttach {
            points: HashMap::from([(attach_id::SHOULDER_LEFT, (3u16, Vec3::ZERO))]),
            markers: HashMap::new(),
        };
        let mut items = HeldItems::default();
        items.slots[4] = Some(HeldSlot {
            display: 7,
            kind: KIND,
            attach: attach_id::SHOULDER_LEFT,
            visual: NO_GLOW,
        });
        let skin = benilla_world::rig_palette::RigSkin::allocate_bones(
            app.world_mut()
                .resource_mut::<benilla_world::rig_palette::RigPalettes>()
                .as_mut(),
            8,
            Handle::default(),
        )
        .expect("a fresh palette has room");
        let wearer_slot = skin.slot;
        let wearer = app
            .world_mut()
            .spawn((items, bones, Transform::default(), skin))
            .id();
        let pose = benilla_world::testing::test_rig_pose(wearer, &[Vec3::ZERO; 4]);
        app.world_mut().entity_mut(wearer).insert(pose);
        app.add_systems(Update, attach_held_items);
        app.update();

        let tags: Vec<u32> = app
            .world_mut()
            .query::<&MeshTag>()
            .iter(app.world())
            .map(|t| t.0)
            .collect();
        // The item root's slot is the one that is not the wearer's.
        let item_rig = app
            .world_mut()
            .query::<&benilla_world::rig_palette::RigSkin>()
            .iter(app.world())
            .map(|r| r.slot)
            .find(|&s| s != wearer_slot);
        let mesh = app
            .world_mut()
            .query::<&Mesh3d>()
            .iter(app.world())
            .map(|m| m.0.clone())
            .next();
        let rider = app
            .world_mut()
            .query::<&benilla_world::rig_rider::RigRider>()
            .iter(app.world())
            .next()
            .copied();
        let joints = app
            .world_mut()
            .query::<&benilla_world::billboard::BillboardJointRig>()
            .iter(app.world())
            .count();
        Attached {
            tags,
            wearer_slot,
            item_rig,
            mesh,
            rider,
            joints,
        }
    }

    fn attach_a_welded_shoulder() -> Attached {
        attach_a_shoulder(true)
    }

    /// Attach one helm; returns the parts' tags and the wearer's slot (0 for a boneless wearer).
    fn attach_a_helm(rigged: bool, interior: bool) -> (Vec<u32>, u16) {
        const HELM_KIND: ItemModelKind = ItemModelKind::Helm { race: 3, sex: 0 };
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // `RigSkin`'s free hook frees the slot through this on teardown.
        app.init_resource::<benilla_world::rig_palette::RigPalettes>();
        let mut displays = ItemDisplays::icons_for_tests(
            benilla_formats::ItemDisplayCatalog::from_displays(HashMap::new()),
        );
        let mut dm = empty_display();
        dm.parts = Some(vec![part(interior)]);
        displays.models.insert((7, HELM_KIND), dm);
        app.insert_resource(displays);

        let bones = BoneAttach {
            points: HashMap::from([(attach_id::HELM, (3u16, Vec3::ZERO))]),
            markers: HashMap::new(),
        };
        let mut items = HeldItems::default();
        items.slots[3] = Some(HeldSlot {
            display: 7,
            kind: HELM_KIND,
            attach: attach_id::HELM,
            visual: NO_GLOW,
        });
        let skin = rigged.then(|| {
            benilla_world::rig_palette::RigSkin::allocate_bones(
                app.world_mut()
                    .resource_mut::<benilla_world::rig_palette::RigPalettes>()
                    .as_mut(),
                8,
                Handle::default(),
            )
            .expect("a fresh palette has room")
        });
        let slot = skin.as_ref().map_or(0, |s| s.slot);
        let mut wearer = app.world_mut().spawn((items, bones, Transform::default()));
        if let Some(skin) = skin {
            wearer.insert(skin);
        }
        let wearer = wearer.id();
        let pose = benilla_world::testing::test_rig_pose(wearer, &[Vec3::ZERO; 4]);
        app.world_mut().entity_mut(wearer).insert(pose);
        app.add_systems(Update, attach_held_items);
        app.update();

        let mut tags: Vec<u32> = app
            .world_mut()
            .query::<&MeshTag>()
            .iter(app.world())
            .map(|t| t.0)
            .collect();
        tags.sort_unstable();
        (tags, slot)
    }

    /// An attached model inherits its parent's computed colours (`0x714000`).
    #[test]
    fn an_attachment_wears_its_wearers_instance_slot() {
        for interior in [true, false] {
            let (tags, slot) = attach_a_helm(true, interior);
            assert!(slot >= 1, "the wearer really has a rig");
            assert_eq!(tags.len(), 1, "one part, one tag (interior={interior})");
            assert_eq!(
                benilla_world::mesh_tag::rig_of(tags[0]),
                slot,
                "the wearer's slot, interior={interior}",
            );
        }
    }

    #[test]
    fn a_part_without_an_interior_variant_still_gets_a_tag() {
        let (tags, _) = attach_a_helm(true, false);
        assert_eq!(tags.len(), 1);
        assert_ne!(tags[0], 0, "not the untagged ⇒ opaque sentinel");
        assert!((benilla_world::mesh_tag::alpha_of(tags[0]) - 1.0).abs() < 1.0 / 63.0);
    }

    #[test]
    fn a_rigless_wearer_leaves_the_slot_at_identity() {
        let (tags, _) = attach_a_helm(false, true);
        assert_eq!(tags.len(), 1);
        assert_eq!(benilla_world::mesh_tag::rig_of(tags[0]), 0);
    }

    /// A display welded to a billboard bone gets its own joint rig (`0x718657`..`0x71876f`).
    #[test]
    fn a_welded_billboard_item_spawns_its_own_rig() {
        let a = attach_a_welded_shoulder();
        let item_slot = a
            .item_rig
            .expect("the welded display allocates a palette slot");
        assert!(a.wearer_slot >= 1, "the wearer really has a rig of its own");
        assert_ne!(
            item_slot, a.wearer_slot,
            "the item's palette is not the wearer's"
        );
        assert_eq!(a.tags.len(), 1);
        assert_eq!(
            benilla_world::mesh_tag::rig_of(a.tags[0]),
            item_slot,
            "the part indexes the ITEM's palette, not the body's"
        );
        assert_eq!(
            a.mesh,
            Some(skinned_handle()),
            "…and draws the skinned twin"
        );
        // A welded display needs the camera-replaced joint rig; a rigid rider frame cannot bend.
        assert_eq!(a.joints, 1, "the welded item runs a billboard joint rig");
        assert!(a.rider.is_none(), "…and is not a rider");
    }

    /// An ordinary item rides one rigid frame in the wearer's rig, never an absolute f32 world
    /// matrix, which at ~9500 yd falls on a 0.98 mm grid.
    #[test]
    fn an_ordinary_item_rides_one_rigid_frame_in_its_wearers_rig() {
        let a = attach_a_shoulder(false);
        let item_slot = a.item_rig.expect("an ordinary item takes a rider slot");
        assert_ne!(item_slot, a.wearer_slot, "its palette is not the wearer's");
        assert_eq!(
            benilla_world::mesh_tag::rig_of(a.tags[0]),
            item_slot,
            "the part indexes the RIDER's palette"
        );
        assert_eq!(
            a.mesh,
            Some(skinned_handle()),
            "…and draws the skinned twin, or the slot would place nothing"
        );
        let rider = a.rider.expect("the item root carries the rider");
        assert_eq!(rider.slot, item_slot);
        assert_eq!(rider.bone, 3, "composed from the wearer's attach bone");
        assert_eq!(rider.local, Vec3::ZERO, "…at the attachment point's offset");
        assert_eq!(a.joints, 0, "a rigid item builds no joint rig");
    }

    /// Only the ranged prop (`[+0xd24]`) gets a pose, a player and emitters on its bone and clock.
    #[test]
    fn a_flexing_ranged_prop_gets_a_pose_a_clock_and_hosted_emitters() {
        fn attach(slot_idx: usize, attach: u16) -> (App, Option<Entity>) {
            let mut app = App::new();
            app.add_plugins(MinimalPlugins);
            app.init_resource::<benilla_world::rig_palette::RigPalettes>();
            app.init_resource::<Assets<bevy::image::Image>>();
            let mut displays = ItemDisplays::icons_for_tests(
                benilla_formats::ItemDisplayCatalog::from_displays(HashMap::new()),
            );
            let mut dm = empty_display();
            let mut p = part(false);
            p.skinned_mesh = Some(skinned_handle());
            dm.parts = Some(vec![p]);
            dm.skeleton = benilla_assets::ModelSkeleton {
                joints: vec![
                    benilla_assets::ModelJoint {
                        parent: -1,
                        local_translation: Vec3::ZERO,
                        billboard: None,
                        parent_arm: None,
                    },
                    // The muzzle bone the emitter hangs off.
                    benilla_assets::ModelJoint {
                        parent: 0,
                        local_translation: Vec3::X,
                        billboard: None,
                        parent_arm: None,
                    },
                ],
                spine_bone: None,
                head_bone: None,
            };
            dm.inverse_bindposes = Some(Handle::default());
            // A firearm's animation shape: Stand + BowRelease, and no BowPull.
            dm.animations = Some(benilla_assets::ModelAnimations {
                graph: Handle::default(),
                clips: Vec::new(),
                hand_close: [None, None],
                playable_animation_lookup: Vec::new(),
                // `owns(161)`, the reference's `0x711960` test.
                animation_lookup: {
                    let mut v = vec![0xffffu16; 162];
                    v[0] = 0;
                    v[crate::ranged_flex::BOW_RELEASE as usize] = 1;
                    v
                },
                global_bones: Vec::new(),
                first_seq: None,
                pose: Default::default(),
            });
            let mut em = benilla_assets::ModelEmitter {
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
            em.def.bone = 1;
            dm.emitters = vec![em];
            displays.models.insert((7, ItemModelKind::Weapon), dm);
            app.insert_resource(displays);

            let bones = BoneAttach {
                points: HashMap::from([(attach, (3u16, Vec3::ZERO))]),
                markers: HashMap::new(),
            };
            let mut items = HeldItems::default();
            items.slots[slot_idx] = Some(HeldSlot {
                display: 7,
                kind: ItemModelKind::Weapon,
                attach,
                visual: NO_GLOW,
            });
            let wearer = app
                .world_mut()
                .spawn((items, bones, Transform::default()))
                .id();
            let pose = benilla_world::testing::test_rig_pose(wearer, &[Vec3::ZERO; 4]);
            app.world_mut().entity_mut(wearer).insert(pose);
            app.add_systems(Update, attach_held_items);
            app.update();
            (app, Some(wearer))
        }

        let (mut app, wearer) = attach(2, attach_id::HAND_RIGHT);
        let wearer = wearer.unwrap();
        let (prop, owner) = app
            .world_mut()
            .query::<(Entity, &crate::ranged_flex::RangedProp)>()
            .iter(app.world())
            .map(|(e, p)| (e, p.owner))
            .next()
            .expect("the ranged prop is marked");
        assert_eq!(owner, wearer, "…and names its wearer, like `[+0xd24]` does");
        assert!(
            app.world().entity(prop).contains::<AnimationPlayer>()
                && app
                    .world()
                    .entity(prop)
                    .contains::<benilla_world::rig_anim::RigPose>(),
            "a flexing prop carries a player and a pose of its own"
        );
        let hosts: Vec<Option<Entity>> = app
            .world_mut()
            .query::<&benilla_world::particles::ParticleEmitter>()
            .iter(app.world())
            .map(|e| e.emit_host())
            .collect();
        assert_eq!(
            hosts,
            vec![Some(prop)],
            "its emitter reads the sequence the PROP is playing"
        );
        let anchors: Vec<(Entity, u16)> = app
            .world_mut()
            .query::<&benilla_world::rig_anim::RigAnchor>()
            .iter(app.world())
            .map(|a| (a.rig, a.bone))
            .collect();
        assert!(
            anchors.contains(&(prop, 1)),
            "the emitter minted the muzzle bone's anchor: {anchors:?}"
        );

        // The same display in the mainhand keeps the resting lane.
        let (mut app, _) = attach(0, attach_id::HAND_RIGHT);
        assert_eq!(
            app.world_mut()
                .query::<&crate::ranged_flex::RangedProp>()
                .iter(app.world())
                .count(),
            0,
            "a melee item is never armed — the reference only ever re-anims the ranged prop"
        );
        let hosts: Vec<Option<Entity>> = app
            .world_mut()
            .query::<&benilla_world::particles::ParticleEmitter>()
            .iter(app.world())
            .map(|e| e.emit_host())
            .collect();
        assert_eq!(
            hosts,
            vec![None],
            "…and its emitter stays on the pinned clock"
        );
    }

    /// A card's booth mirror seats at the attach point plus its pivot, the emitters' at the attach
    /// point; both nonzero, so either term missing fails.
    #[test]
    fn an_equipped_items_card_and_emitters_are_published_for_the_booths() {
        const SHOULDER_KIND: ItemModelKind = ItemModelKind::ShoulderRight;
        const ATTACH: Vec3 = Vec3::new(0.21, 1.42, 0.06);
        const PIVOT: Vec3 = Vec3::new(-0.06, 0.162, -0.012);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<benilla_world::rig_palette::RigPalettes>();
        let mut displays = ItemDisplays::icons_for_tests(
            benilla_formats::ItemDisplayCatalog::from_displays(HashMap::new()),
        );
        let mut dm = empty_display();
        // An R14 pauldron's shape: a plain mesh batch, a camera-facing batch, and emitters.
        let mut card = part(false);
        card.billboard = Some(benilla_assets::BillboardInfo {
            pivot: PIVOT,
            bone: 1,
            kind: benilla_formats::BillboardKind::Spherical,
            scale_anim: None,
            seq_translations: Vec::new(),
        });
        dm.parts = Some(vec![part(false), card]);
        dm.emitters = vec![benilla_assets::ModelEmitter {
            def: benilla_world::testing::plain_particle_def(),
            texture: None,
            bone_pivot: [0.0; 3],
            billboard: Some(benilla_assets::EmitterBillboard {
                kind: benilla_formats::BillboardKind::Spherical,
                pivot: [0.0; 3],
                bone: 1,
            }),
            recursion: None,
            geometry: None,
            owner_reach: 0.0,
            water_bound: (Vec3::ZERO, 0.0),
            idle_seq: 0,
        }];
        displays.models.insert((7, SHOULDER_KIND), dm);
        app.insert_resource(displays);

        let bones = BoneAttach {
            points: HashMap::from([(attach_id::SHOULDER_RIGHT, (3u16, ATTACH))]),
            markers: HashMap::new(),
        };
        let mut items = HeldItems::default();
        items.slots[4] = Some(HeldSlot {
            display: 7,
            kind: SHOULDER_KIND,
            attach: attach_id::SHOULDER_RIGHT,
            visual: NO_GLOW,
        });
        let wearer = app
            .world_mut()
            .spawn((items, bones, Transform::default()))
            .id();
        let pose = benilla_world::testing::test_rig_pose(wearer, &[Vec3::ZERO; 4]);
        app.world_mut().entity_mut(wearer).insert(pose);
        app.add_systems(Update, attach_held_items);
        app.update();

        let mut cards = app
            .world_mut()
            .query::<&crate::portrait::PortraitBillboard>();
        let published: Vec<_> = cards.iter(app.world()).collect();
        assert_eq!(published.len(), 1, "one camera-facing batch, one mirror");
        assert_eq!(published[0].bone, 3, "the BODY bone, not the item's bone 1");
        assert_eq!(
            published[0].seat,
            crate::portrait::PortraitSeat::Rider(ATTACH + PIVOT),
            "a rig-less rider's card carries attach + its own pivot",
        );

        let mut fx = app.world_mut().query::<&crate::portrait::PortraitEffects>();
        let published: Vec<_> = fx.iter(app.world()).collect();
        assert_eq!(published.len(), 1, "one effect-bearing model, one mirror");
        assert_eq!(published[0].bone, 3);
        assert_eq!(
            published[0].offset, ATTACH,
            "the host seats at the attach point; each emitter's own pivot is applied inside",
        );
        assert_eq!(published[0].emitters.len(), 1);
        assert!(
            published[0].emitters[0].billboard.is_some(),
            "the billboard-chain arm survives the carry — it is what the booth builds a frame from",
        );
    }

    /// A card joins the wearer's pending fade and keeps its authored 0.30 weight.
    #[test]
    fn an_items_card_joins_the_wearers_fade_and_keeps_its_authored_alpha() {
        const KIND: ItemModelKind = ItemModelKind::Weapon;
        const SINCE: f32 = 3.0;
        let blend: Handle<benilla_assets::materials::WowModelMaterial> = Handle::Uuid(
            bevy::asset::uuid::Uuid::from_u128(99),
            std::marker::PhantomData,
        );
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<benilla_world::rig_palette::RigPalettes>();
        let mut displays = ItemDisplays::icons_for_tests(
            benilla_formats::ItemDisplayCatalog::from_displays(HashMap::new()),
        );
        let mut dm = empty_display();
        let mut card = part(false);
        card.fade_blend = Some(blend.clone());
        card.alpha_anim = benilla_formats::AlphaAnim::new(vec![benilla_formats::AlphaSeq {
            color: None,
            weight: Some(benilla_formats::ScalarAnim {
                period: 0.0,
                step: false,
                wrap: true,
                gseq: false,
                keys: vec![(0.0, 0.3)],
            }),
        }])
        .map(std::sync::Arc::new);
        card.billboard = Some(benilla_assets::BillboardInfo {
            pivot: Vec3::ZERO,
            bone: 1,
            kind: benilla_formats::BillboardKind::Spherical,
            scale_anim: None,
            seq_translations: Vec::new(),
        });
        dm.parts = Some(vec![card]);
        displays.models.insert((7, KIND), dm);
        app.insert_resource(displays);

        let bones = BoneAttach {
            points: HashMap::from([(attach_id::HAND_RIGHT, (3u16, Vec3::ZERO))]),
            markers: HashMap::new(),
        };
        let mut items = HeldItems::default();
        items.slots[0] = Some(HeldSlot {
            display: 7,
            kind: KIND,
            attach: attach_id::HAND_RIGHT,
            visual: NO_GLOW,
        });
        // The wearer's own fade is still pending.
        let wearer = app
            .world_mut()
            .spawn((
                items,
                bones,
                Transform::default(),
                UnitAppearFade::Pending { since: SINCE },
            ))
            .id();
        let pose = benilla_world::testing::test_rig_pose(wearer, &[Vec3::ZERO; 4]);
        app.world_mut().entity_mut(wearer).insert(pose);
        app.add_systems(Update, attach_held_items);
        app.update();

        let mut q = app.world_mut().query::<(
            &BillboardCard,
            &MeshTag,
            &MeshMaterial3d<benilla_assets::materials::WowModelMaterial>,
            Option<&benilla_world::model_fade::FadeMaterials>,
            Option<&benilla_world::model_fade::PendingAppearFade>,
            Option<&benilla_world::doodad_anim::MatAnim>,
        )>();
        let found: Vec<_> = q.iter(app.world()).collect();
        assert_eq!(found.len(), 1, "the one camera-facing batch spawned a card");
        let (_, tag, mat, fm, pending, anim) = found[0];
        assert_eq!(
            pending.map(|p| p.since),
            Some(SINCE),
            "the card joined the wearer's pending ramp",
        );
        assert_eq!(
            fm.map(|f| f.blend.clone()),
            Some(blend.clone()),
            "…carrying the material record the ramp (and the zoom feather) re-arm from",
        );
        assert_eq!(mat.0, blend, "…and opens ON the blend twin, not the cutout");
        assert!(
            benilla_world::mesh_tag::alpha_of(tag.0) <= 1.0 / 63.0,
            "…at the encoder's ≈0 floor, so it never flashes opaque for a frame",
        );
        let anim = anim.expect("the batch's authored alpha rides the card");
        assert!(
            (anim.current - 0.3).abs() < 1e-6,
            "the file's 0.30 weight, not 1.0",
        );
        assert!(
            anim.composes_unit_tag(),
            "an attach model's compose is the UNIT lane's — ordered against the wearer's fade",
        );
    }

    /// A Mod2x sheen reads no alpha, so it fades on its steady material while the shader lerps its
    /// colour toward the blend identity by the tag alpha, the reference's preset-5 fade.
    #[test]
    fn a_multiply_sheen_joins_the_wearers_ramp_on_its_steady_material() {
        const KIND: ItemModelKind = ItemModelKind::Weapon;
        const SINCE: f32 = 3.0;
        let steady: Handle<benilla_assets::materials::WowModelMaterial> = Handle::Uuid(
            bevy::asset::uuid::Uuid::from_u128(0x5133),
            std::marker::PhantomData,
        );
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<benilla_world::rig_palette::RigPalettes>();
        let mut displays = ItemDisplays::icons_for_tests(
            benilla_formats::ItemDisplayCatalog::from_displays(HashMap::new()),
        );
        let mut dm = empty_display();
        let mut sheen = part(false);
        sheen.blend = benilla_formats::ModelBlend::Mod2x;
        sheen.material = steady.clone();
        // As `entities::display` builds a multiply batch: the steady self as the twin.
        sheen.fade_blend = Some(steady.clone());
        dm.parts = Some(vec![sheen]);
        displays.models.insert((7, KIND), dm);
        app.insert_resource(displays);

        let bones = BoneAttach {
            points: HashMap::from([(attach_id::HAND_RIGHT, (3u16, Vec3::ZERO))]),
            markers: HashMap::new(),
        };
        let mut items = HeldItems::default();
        items.slots[0] = Some(HeldSlot {
            display: 7,
            kind: KIND,
            attach: attach_id::HAND_RIGHT,
            visual: NO_GLOW,
        });
        let wearer = app
            .world_mut()
            .spawn((
                items,
                bones,
                Transform::default(),
                UnitAppearFade::Pending { since: SINCE },
            ))
            .id();
        let pose = benilla_world::testing::test_rig_pose(wearer, &[Vec3::ZERO; 4]);
        app.world_mut().entity_mut(wearer).insert(pose);
        app.add_systems(Update, attach_held_items);
        app.update();

        let mut q = app.world_mut().query::<(
            &MeshTag,
            &MeshMaterial3d<benilla_assets::materials::WowModelMaterial>,
            Option<&benilla_world::model_fade::FadeMaterials>,
            Option<&benilla_world::model_fade::PendingAppearFade>,
        )>();
        let found: Vec<_> = q.iter(app.world()).collect();
        assert_eq!(found.len(), 1, "the one sheen batch spawned");
        let (tag, mat, fm, pending) = found[0];
        assert_eq!(
            pending.map(|p| p.since),
            Some(SINCE),
            "the sheen joined the wearer's pending ramp (it used to spawn Steady and pop)",
        );
        assert_eq!(
            mat.0, steady,
            "no material swap — the steady multiply pipeline"
        );
        let fm = fm.expect("a FadeMaterials record, so despawn/stealth ramps re-arm it");
        assert_eq!(fm.blend, steady, "…whose 'twin' is the steady self");
        assert!(
            benilla_world::mesh_tag::alpha_of(tag.0) <= 1.0 / 63.0,
            "…opening at tag alpha ≈ 0: the shader's identity-lerp makes it contribute nothing",
        );
    }

    /// A wearer with a drawn mainhand weapon and a shoulder that carries emitters.
    fn dress_a_wearer() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<benilla_world::rig_palette::RigPalettes>();
        let mut displays = ItemDisplays::icons_for_tests(
            benilla_formats::ItemDisplayCatalog::from_displays(HashMap::new()),
        );
        for (display, kind, effects) in [
            (7, ItemModelKind::Weapon, false),
            (9, ItemModelKind::Weapon, false),
            (8, ItemModelKind::ShoulderRight, true),
        ] {
            let mut dm = empty_display();
            let mut p = part(false);
            // A skeleton and a skinned twin, so the item takes a rider slot.
            p.skinned_mesh = Some(skinned_handle());
            dm.parts = Some(vec![p]);
            dm.skeleton = benilla_assets::ModelSkeleton {
                joints: vec![benilla_assets::ModelJoint {
                    parent: -1,
                    local_translation: Vec3::ZERO,
                    billboard: None,
                    parent_arm: None,
                }],
                spine_bone: None,
                head_bone: None,
            };
            if effects {
                dm.emitters = vec![benilla_assets::ModelEmitter {
                    def: benilla_world::testing::plain_particle_def(),
                    texture: None,
                    bone_pivot: [0.0; 3],
                    billboard: None,
                    recursion: None,
                    geometry: None,
                    owner_reach: 0.0,
                    water_bound: (Vec3::ZERO, 0.0),
                    idle_seq: 0,
                }];
            }
            displays.models.insert((display, kind), dm);
        }
        app.insert_resource(displays);

        // The hand, a back sheath point and the shoulder, on three bones.
        let bones = BoneAttach {
            points: HashMap::from([
                (attach_id::HAND_RIGHT, (1u16, HAND_AT)),
                (SHEATH_BACK, (2u16, BACK_AT)),
                (attach_id::SHOULDER_RIGHT, (3u16, SHOULDER_AT)),
            ]),
            markers: HashMap::new(),
        };
        let mut items = HeldItems::default();
        items.slots[0] = Some(HeldSlot {
            display: 7,
            kind: ItemModelKind::Weapon,
            attach: attach_id::HAND_RIGHT,
            visual: NO_GLOW,
        });
        items.slots[4] = Some(HeldSlot {
            display: 8,
            kind: ItemModelKind::ShoulderRight,
            attach: attach_id::SHOULDER_RIGHT,
            visual: NO_GLOW,
        });
        let wearer = app
            .world_mut()
            .spawn((items, bones, Transform::default()))
            .id();
        let pose = benilla_world::testing::test_rig_pose(wearer, &[Vec3::ZERO; 4]);
        app.world_mut().entity_mut(wearer).insert(pose);
        app.add_systems(Update, attach_held_items);
        app.update();
        (app, wearer)
    }

    const HAND_AT: Vec3 = Vec3::new(0.1, 1.0, 0.0);
    const BACK_AT: Vec3 = Vec3::new(-0.2, 1.3, -0.15);
    const SHOULDER_AT: Vec3 = Vec3::new(0.21, 1.42, 0.06);
    /// Stands in for a back sheath point: any id the body publishes will do.
    const SHEATH_BACK: u16 = 6;

    fn roots_of(app: &App, wearer: Entity) -> [Option<Entity>; ATTACH_SLOTS] {
        app.world()
            .entity(wearer)
            .get::<HeldAttached>()
            .unwrap()
            .spawned
    }

    /// A sheath swap re-parents the weapon's root, and no other slot's root is rebuilt.
    #[test]
    fn a_sheath_swap_moves_the_weapon_and_leaves_the_other_slots_alone() {
        let (mut app, wearer) = dress_a_wearer();
        let before = roots_of(&app, wearer);
        let (weapon, shoulder_root) = (before[0].expect("weapon"), before[4].expect("shoulder"));

        // Stow it: the same item at a new attach point.
        let mut items = app.world_mut().entity_mut(wearer);
        let mut items = items.get_mut::<HeldItems>().unwrap();
        items.slots[0].as_mut().unwrap().attach = SHEATH_BACK;
        app.update();

        let after = roots_of(&app, wearer);
        assert_eq!(
            after[0],
            Some(weapon),
            "the weapon MOVED — same model instance"
        );
        assert_eq!(
            after[4],
            Some(shoulder_root),
            "an untouched slot is not rebuilt by someone else's sheath swap"
        );
        assert!(
            app.world().get_entity(shoulder_root).is_ok(),
            "…and its root really is alive: every effect riding it survives the swap"
        );
        assert_eq!(
            app.world()
                .entity(weapon)
                .get::<ChildOf>()
                .map(|c| c.parent()),
            app.world()
                .entity(wearer)
                .get::<benilla_world::rig_anim::RigPose>()
                .unwrap()
                .anchors
                .iter()
                .find(|&&(b, _)| b == 2)
                .map(|&(_, j)| j),
            "re-parented onto the sheath point's joint"
        );
        assert_eq!(
            app.world()
                .entity(weapon)
                .get::<Transform>()
                .unwrap()
                .translation,
            BACK_AT,
            "…at the new attach point's offset"
        );
        let seat = app
            .world_mut()
            .query::<&crate::portrait::PortraitRider>()
            .iter(app.world())
            .find(|r| r.offset.distance(BACK_AT) < 1e-5)
            .map(|r| r.bone);
        assert_eq!(seat, Some(2), "the rider's cached seat followed the move");
        // The palette rider follows too, or the sword would draw at the hand it left.
        let rider = app
            .world()
            .entity(weapon)
            .get::<benilla_world::rig_rider::RigRider>()
            .expect("the weapon rides a rider frame");
        assert_eq!(rider.bone, 2, "the rider is composed from the sheath bone");
        assert_eq!(rider.local, BACK_AT, "…at the sheath point's offset");
        assert_eq!(rider.host, wearer);
    }

    #[test]
    fn a_real_item_change_rebuilds_only_its_own_slot() {
        let (mut app, wearer) = dress_a_wearer();
        let before = roots_of(&app, wearer);
        let (weapon, shoulder_root) = (before[0].expect("weapon"), before[4].expect("shoulder"));

        let mut items = app.world_mut().entity_mut(wearer);
        let mut items = items.get_mut::<HeldItems>().unwrap();
        items.slots[0].as_mut().unwrap().display = 9;
        app.update();

        let after = roots_of(&app, wearer);
        assert!(
            after[0].is_some_and(|e| e != weapon),
            "a different display is a different model: rebuilt"
        );
        assert!(
            app.world().get_entity(weapon).is_err(),
            "the old model is destroyed, not left behind"
        );
        assert_eq!(after[4], Some(shoulder_root), "the shoulders are untouched");
    }

    #[test]
    fn an_item_model_is_chained_to_its_wearer() {
        use benilla_world::model_fade::ParentModel;

        let (mut app, wearer) = dress_a_wearer();
        let shoulder = roots_of(&app, wearer)[4].expect("shoulder");
        assert_eq!(
            app.world()
                .entity(shoulder)
                .get::<ParentModel>()
                .map(|p| p.0),
            Some(wearer),
            "the item chains to the body wearing it"
        );

        let mut items = app.world_mut().entity_mut(wearer);
        let mut items = items.get_mut::<HeldItems>().unwrap();
        items.slots[0].as_mut().unwrap().attach = SHEATH_BACK;
        app.update();
        assert_eq!(
            app.world()
                .entity(shoulder)
                .get::<ParentModel>()
                .map(|p| p.0),
            Some(wearer),
            "…and a sheath swap elsewhere leaves that link alone"
        );
    }
}
