//! Equipment **attach** (decisions 0072/0074, split out of `super`'s one file): the sub-model
//! children a unit's resolved [`HeldItems`] spawns — each item model's parts under its attach
//! point's joint entity, plus everything that rides an item (its billboard cards, emitters,
//! lights, ribbons, and the glow its `ItemVisuals` id names — decision 0805).

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

/// The three material handles one attach-model batch fades through — the item lane's
/// [`FadeSet`], read straight off the built part (an item batch never carries a character
/// runtime slot the way a body batch does).
fn item_fade_set(part: &super::super::EntityPart) -> FadeSet<'_> {
    FadeSet {
        steady: &part.material,
        blend: part.fade_blend.as_ref(),
        bake_blend: part.material_interior_bake_blend.as_ref(),
        zfill: part.zfill.as_ref(),
    }
}

/// Everything one slot's spawn needs from its WEARER, read once per unit — the context
/// [`spawn_slot`] carries so the per-slot body isn't a fifteen-argument call.
struct WearerCtx<'a> {
    /// The unit wearing the item — the parent whose tint, light collector and fade it inherits.
    wearer: Entity,
    bones: &'a BoneAttach,
    /// The unit's appear-fade clock, for a part spawning mid-ramp ([`join_unit_appear_fade`]).
    joined: JoinedFade,
    now: f32,
    /// The wearer's rig-palette slot, pre-shifted into `MeshTag` bits (decision 0812).
    rig_slot: u16,
    /// The wearer's body bake centre, when it has one — the interior classifier's fold reference.
    body_center: Option<Vec3>,
    /// The wire scale the unit renders at: the world yards its held effects' draw-order rung is
    /// measured in ([`benilla_world::particles::owner_last_bias`]).
    scale: f32,
}

/// The **re-seat writers** (decision 0826): everything under an item root that caches *where on the
/// body* the item sits. A pure attach-point change — the sheath swap — MOVES the root instead of
/// rebuilding it, and these move by the same delta, so the item's glow instances, its effect hosts
/// and its live particle clouds all ride along instead of being orphaned and respawned.
#[derive(bevy::ecs::system::SystemParam)]
pub(in crate::entities) struct SeatWriters<'w, 's> {
    children: Query<'w, 's, &'static Children>,
    riders: Query<'w, 's, &'static mut crate::portrait::PortraitRider>,
    cards: Query<'w, 's, &'static mut crate::portrait::PortraitBillboard>,
    effects: Query<'w, 's, &'static mut crate::portrait::PortraitEffects>,
    glows: Query<'w, 's, &'static mut ItemGlow>,
}

impl SeatWriters<'_, '_> {
    /// Move every cached seat in `root`'s subtree to the item's new attach point: the body `bone`
    /// it now hangs from, and `delta` = `new_offset − old_offset` applied to each cached offset.
    ///
    /// The delta (rather than an assignment) is what makes this total: a mirror's offset is the
    /// attach point *plus* something model-local — a card's own pivot, a glow slot's point on the
    /// item — and only the attach-point term moves. The walk is recursive because an item's glow
    /// instances hang two levels down.
    ///
    /// `attach` is assigned, not delta'd: it names WHICH attachment node the item now sits in, and
    /// that is what a widget's attach reset filters on ([`crate::portrait::attach_reset`]). A move
    /// that left it stale would keep, say, a stowed weapon marked with the hand id it left.
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

/// Spawn/refresh the held-item children for every unit whose [`HeldItems`] changed (or whose item
/// model finished loading): each slot's model parts spawn under the attach point's joint entity at
/// the attachment offset, so they ride the bone. Slots whose model is still loading are left pending
/// (the `applied` diff key keeps them un-applied) and picked up on a later pass. A part spawning while
/// the unit's own appear-fade is still in flight joins it ([`join_unit_appear_fade`]) instead of
/// popping in opaque (decision 0032 read as a per-unit property).
///
/// **The diff is per SLOT, and an attach-point change is a MOVE** (decision 0826). The reference's
/// sheath paths touch only the weapon/quiver attach ids (`0x611770`) and stow a melee weapon by
/// detaching the sub-model and **re-parenting it** at the sheath point (`0x60b590` → `0x712f70`) —
/// the model instance, and everything riding it, survives the swap. Rebuilding a unit's whole kit
/// on any change did neither: drawing a sword respawned the shoulders' and helm's emitters too, and
/// every orphaned pool then lived out its lifespan FROZEN in world space (`particles::sim`'s drain)
/// — the sparkle cloud that hung behind the character on every weapon draw and every step of the
/// login gear cascade.
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
        // The unit root's own transform — its scale is the wire `NetEntity::scale` the streamer
        // writes (`entities::attach`), and the held effects' draw-order rung is measured in the
        // world yards that scale produces.
        Option<&Transform>,
        // The WEARER's rig, for its instance slot — the fallback an attached model's parts carry
        // in their tag when they have no slot of their own, so the wearer's body tint still
        // reaches them (decision 0812 — the reference's attached models inherit the parent CM2's
        // computed colours, `0x714000`). An item that owns a slot carries its own instead and
        // inherits the tint up the `ParentModel` chain: the `welds_billboard` joint rig (0841),
        // and — since 1609 — every ordinary attached model, which rides one rigid palette frame
        // composed in this rig's frame rather than an absolute world matrix.
        Option<&benilla_world::rig_palette::RigSkin>,
        // The wearer's pose buffer: the attach joint spawns on first demand from the composed
        // pose (`RigPose::anchor_for`, decision 1355) — a weapon equipped in combat seats at the
        // live pose, never the rest pose.
        Option<&mut benilla_world::rig_anim::RigPose>,
    )>,
    held: Option<Res<ItemDisplays>>,
    time: Res<Time>,
    mut seats: SeatWriters,
    // The MOVE path re-seats a rider in place: an attach-point change is the one edit that leaves
    // the item's entity alive while changing the bone its palette frame is composed from (1609).
    mut movers: Query<&mut benilla_world::rig_rider::RigRider>,
    // Every attached model with a skeleton allocates a palette slot of its own: one rigid frame in
    // the wearer's rig frame (1609), or — for a display that welds geometry to a billboard bone,
    // which has no correct rigid placement at all — its own camera-replaced joint rig (0841).
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
) {
    let Some(held) = held else {
        return;
    };
    let now = time.elapsed_secs();
    for (items, bones, attached, entity, unit_fade, body_center, unit_tf, skin, mut pose) in
        &mut units
    {
        // A held item / helm / shoulder resolves and spawns asynchronously (a template round trip, a
        // model load) — often *after* the body has already armed its appear-fade (decision 0032 is a
        // per-unit property: the reference fades the whole unit, attachments included, as one). Read
        // the unit root's fade clock once per unit so every part spawned below joins the same ramp
        // instead of popping in opaque.
        let ctx = WearerCtx {
            wearer: entity,
            bones,
            joined: join_unit_appear_fade(unit_fade.copied()),
            now,
            rig_slot: skin.map_or(0, |rb| rb.slot),
            body_center: body_center.map(|c| c.0),
            scale: unit_tf.map_or(1.0, |t| t.scale.max_element()),
        };
        // Diff against what's spawned; skip when unchanged. A slot whose model hasn't built parts yet
        // is masked out of `next` so it stays "not yet applied" and re-attaches once the parts exist.
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
            // The MOVE: same item, new attach point (the sheath swap). Everything riding the root
            // — meshes, cards, glow instances, lights, ribbons and the live particle pools that
            // owner-follow it — comes along, and the cached booth seats shift with it.
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
                            // The rider's frame is composed from (bone, offset), so the move has
                            // to carry it — otherwise a sheathed sword keeps drawing at the hand
                            // it was drawn from while its entity, its glow and its emitters all
                            // travel to the hip (1609).
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
            // Otherwise this slot really is a different item (or none): tear the old one down —
            // model gone, effects gone with it — and build the new one.
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

/// Spawn one slot's item model under its attach point, with everything that rides it: the mesh
/// parts, the camera-facing cards, the booth mirrors, the item glow, the emitters, the lights and
/// the ribbons. `None` when the display has no built parts or the body has no such attach point.
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
    // Body model has no such attach point (a non-character skeleton) — hold nothing.
    let &(bone, offset) = bones.points.get(&hs.attach)?;
    // The attach joint, spawned on first demand from the wearer's composed pose (decision 1355).
    let joint = pose?.anchor_for(commands, entity, bone)?;
    let root = commands
        .spawn((
            Transform::from_translation(offset),
            Visibility::default(),
            // This item model is CHAINED to the body wearing it (`0x712f70` attach → the
            // `[model+0x1cc]` parent link): the wearer's computed render alpha multiplies
            // everything this root carries, and everything chained below it in turn — its glow
            // instances included (decision 0833).
            benilla_world::model_fade::ParentModel(entity),
        ))
        // Chain-only visibility (benilla_world::vis_chain): the wrapper renders nothing —
        // the item's parts and glows are the children.
        .vis_chain_only()
        .id();
    commands.entity(joint).add_child(root);
    // The item/enchant glow (decision 0805): its instances hang off the ITEM's own
    // attachment points, so they are children of this root — spawned by
    // [`super::item_glow::attach_item_glows`] once the glow models build, and reaped with
    // the root on any gear/sheath change, which is the whole lifetime rule.
    if hs.visual != NO_GLOW {
        commands
            .entity(root)
            .insert(super::super::item_glow::ItemGlow {
                display: hs.display,
                kind: hs.kind,
                visual: hs.visual,
                // The item's seat on the BODY, carried so the glow attach can publish its own
                // booth mirrors at a seat composed from it (decision 0822) — the glow spawns
                // asynchronously and knows only this root. The attach id rides along for the
                // same reason: a glow is chained UNDER the item, so the reference's attach reset
                // takes it exactly when it takes the item ([`crate::portrait::attach_reset`]).
                bone,
                offset,
                attach: hs.attach,
            });
    }
    // The engine-drawn bowstring (0408 §G2) — for the drawn BOW only: the ranged slot's
    // left-hand fork (a bow is the one ranged weapon placed in HAND_LEFT; the client
    // registers the string callback bow-only, from the ranged-draw path). The `$WTT`/
    // `$WTB` anchors alone are NOT the gate — they are generic weapon-TRAIL begin/end
    // markers (`WTBT`/`WTTT`, the swing-trail vertex build `0x6c67f0`) that melee
    // weapons author too; keying on their presence drew a phantom "bowstring" chord
    // across the Whirlwind Axe's blade tips (decision 0531).
    if slot_idx == 2 && hs.attach == attach_id::HAND_LEFT {
        if let Some([top, bottom]) = dm.string_anchors {
            commands.entity(root).insert(crate::bowstring::Bowstring {
                owner: entity,
                // Bone AND offset: the prop flexes, so the tips move (decision 2281).
                top,
                bottom,
            });
        }
    }
    // The **weapon swing trail's** object (decision 2076)
    // — the reference's `WTOBJECT`, built per weapon HAND by `0x608d60` on the model it finds at
    // that hand's attachment and freed with it. This is the first-class consumer decision 0531
    // named and deferred: the `$WTB`/`$WTT` pair is a *trail* marker, and the bowstring above is
    // the special case, not the rule. `0x6c67f0`'s `0x7130e0` presence gate is exactly the
    // `string_anchors` `Option` — a weapon authoring only one marker draws nothing at all.
    if slot_idx <= 1 {
        if let Some([top, bottom]) = dm.string_anchors {
            commands
                .entity(root)
                .insert(crate::weapon_trail::WeaponTrail::new(top.1, bottom.1));
        }
    }
    // The fishing line's near anchor (`0x61f780`, decision 1099): a MAINHAND prop
    // whose model authors `$CCH` is the pole (the reference gates on ItemCache {class 2, subclass
    // 20} + the marker's presence; exactly one weapon model in the chain authors the marker, so
    // presence alone is data-equivalent — unlike the bow's trail-marker trap above, which is why
    // the bowstring can't key the same way). The drawer spans it to the owner's channel bobber.
    if slot_idx == 0 {
        if let Some(tip) = dm.cch_marker {
            commands
                .entity(root)
                .insert(crate::fishing_line::FishingPoleTip { owner: entity, tip });
        }
    }
    // **The RANGED PROP's own animation** (decision 2281) — benilla's `[CGUnit+0xd24]`. The
    // reference keeps the equipped ranged weapon's M2 *instance* and re-arms its animation from the
    // body's `$BWP`/`$BWR` keyframes: BowPull(160) on the pull, and on the release either Stand(0)
    // (a bow's limbs relax) or **BowRelease(161)** — which on a firearm is the muzzle blast, seven
    // particle emitters keyed to zero in Stand and to a 0.2 s burst in 161. See
    // [`crate::ranged_flex`] for the byte law and the corpus measurement.
    //
    // So this one held model is NOT the lane's "rests at bind pose" case, and it earns the three
    // things a resting attach model is denied: a pose buffer, a player, and bone ANCHORS for its
    // emitters (bone 2 of every firearm is keyed +90° in 161 — that rotation is what aims the blast
    // down the barrel). The gate is the asset: 46 of the 571 shipped weapon models author 160 or
    // 161, all of them bows, crossbows and firearms, and nothing else in the corpus does.
    let flexes = slot_idx == 2
        && !dm.skeleton.joints.is_empty()
        && dm.animations.as_ref().is_some_and(|a| {
            a.owns(crate::ranged_flex::BOW_PULL) || a.owns(crate::ranged_flex::BOW_RELEASE)
        });
    // **The item rig** (decisions 0841, withdrawn by 0847, RESTORED by 0854) — the one case an
    // attach model runs a joint palette. 0847 pulled it believing a spherical billboard swept the
    // spikes through the plate; that was wrong (0853: the spikes run ALONG their bone, worst vertex
    // 12° off axis, so the arc never existed), and the byte answer is that the reference billboards
    // an attached model exactly as it does a standalone one (`0x718657`–`0x71876f`).
    // A
    // display whose geometry is WELDED to a billboard bone (`welds_billboard`: the R14 pauldron's
    // two spikes, whose root rings are 50/50 with the plate) has no correct rigid placement at all;
    // 0839 stopped tearing it into a card, which left it whole and *still*. The reference bends it
    // by blending the billboard bone's camera-replaced palette row per vertex (`0x71a460`) — so
    // we build exactly that: the joint hierarchy, a palette slot, and the
    // [`benilla_world::billboard::BillboardJointRig`] that rewrites the billboard joints' world rotations.
    //
    // Nothing else about the item lane's "rests at bind pose" law moves: no `AnimationPlayer`, no
    // global-sequence drive — all seven affected models are **keyless** (`m2bones`: not one
    // T/R/S key between them), so every non-billboard joint's palette row is the identity the
    // static mesh already drew, and the emitters/lights/ribbons below keep riding `root`.
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
            // Slot 0 (table full, warned once) ⇒ no rig: the parts below fall back to the static
            // mesh and the wearer's slot, i.e. exactly the pre-0841 look. Never a crash, never a
            // missing shoulder.
            let rig = benilla_world::rig_palette::RigSkin::allocate(palettes, joints, ibp.clone())?;
            let slot = rig.slot;
            commands.entity(root).insert(rig);
            // The lane is otherwise invisible from outside the renderer — seven models in 9691 take
            // it, and a run where the gate silently stopped firing looks exactly like a run where
            // nothing was equipped. Naming the slot is what lets a probe say which it was.
            debug!(
                "item rig: display {} welds a billboard bone → palette slot {slot} ({} bones)",
                hs.display,
                dm.skeleton.joints.len()
            );
            Some(slot)
        });
    // **The rider** (decision 1609) — the other, and now ordinary, reason an attached model owns a
    // palette slot. It is not the item's skeleton: an item rests at bind pose, so every row of it
    // is the same placement matrix, and the lane writes that ONE frame — composed in the WEARER's
    // rig frame, with the wearer's world position riding the slot's `rig_origin`.
    //
    // The picture is unchanged by construction (bind pose ⇒ `row = F` ⇒ the static mesh's own
    // placement); what changes is that the placement is never an absolute f32 world coordinate.
    // Built as a `~9500 + ~1` sum every frame, it landed on the 0.98 mm ULP grid of the large map
    // axis and the helm snapped a whole ULP sideways every other frame while the body under it
    // moved exactly — the jitter the director reported on the Goldshire guard. See
    // [`benilla_world::rig_rider`] for the measurement and the residuals.
    //
    // A boneless display has no skinned twin to draw and keeps the old route; so does a palette
    // with no room, which is the same graceful fallback the `welds_billboard` lane already had.
    let rider = match item_rig {
        Some(_) => None,
        // A slot only earns its keep if something will actually SKIN through it: a display with
        // bones but no skinned twin on any drawn part (a card-only model) would take a slot that
        // no vertex ever indexes, and keep the absolute placement anyway.
        None => (!dm.skeleton.joints.is_empty()
            && parts
                .iter()
                .any(|p| p.billboard.is_none() && p.skinned_mesh.is_some()))
        .then(|| {
            benilla_world::rig_palette::RigSkin::allocate_bones(
                palettes,
                dm.skeleton.joints.len() as u32,
                // A resting rider needs no inverse bindposes — at bind pose every row collapses to
                // the placement and 1609's write never consults them. A FLEXING prop's rows are
                // `F × model[b] × ibp[b]`, so its slot carries the real handle.
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
    // The flexing prop's own pose buffer and clock (see `flexes` above). The collapsed lane
    // ([`benilla_world::rig_anim::RigPose`] — no joint entities; bone anchors are minted on demand
    // by the emitter loop below), armed on the model-load bootstrap the reference gives every M2
    // instance: animation id 0 through the model's own `playableAnimationLookup`. From there
    // [`crate::ranged_flex`] re-arms it per keyframe.
    //
    // Its palette rows stay the RIDER lane's — `RigRider` is still on this root, and 2281 taught
    // that lane to compose a pose rather than repeat one frame. That is deliberate: composing this
    // prop from its own absolute `GlobalTransform` instead would hand the bow back the 0.98 mm
    // one-ULP staircase 1609 exists to have removed (the attach point is a ~1 yd quantity summed
    // onto a ~9500 yd one, recomputed every frame).
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
    // The instance slot every part below carries in its `MeshTag`. The WEARER's only when the item
    // has no palette slot of its own — that is what puts a tinted body's colour on a boneless
    // helm (decision 0812). An item WITH a slot must carry its own, because the vertex stage
    // indexes the palette with the same field; the wearer's tint reaches it through the
    // `ParentModel` chain instead, which is the reference's own route for an attached model's
    // colours (`0x714000`, `aura_visual::chained_tint`).
    let rig_slot = item_rig.or(rider).unwrap_or(ctx.rig_slot);
    // Billboard batches (the torch's glow card) collected for the world-root card spawn
    // below — as plain children they'd render at the item root (the grip), not the
    // authored pivot (the torch head). Decision 0153.
    let mut billboard_parts = Vec::new();
    commands.entity(root).with_children(|parent| {
        for part in parts {
            if let Some(info) = &part.billboard {
                billboard_parts.push((info.clone(), part));
                continue;
            }
            // Per-part join decision: `joined` (the unit's clock) combined with whether this
            // part is fade-capable at all. A pending part opens on the blend twin at ≈0 and a
            // joiner at the ramp's *current* alpha, so neither flashes for a frame.
            let set = item_fade_set(part);
            let effective = PartFade::resolve(joined, &set);
            let (init_mat, tag_alpha) = effective.seed(&set, now);
            // An item with a palette slot draws the SKINNED twin — every part of it, not only a
            // welded batch: the model rests at bind pose, so each non-billboard bone's palette row
            // is the placement matrix itself and the batches land pixel-for-pixel where the static
            // mesh had them. Slotless (boneless display, or a full palette) falls back to `mesh`.
            let skinned = item_rig.or(rider).and(part.skinned_mesh.as_ref());
            let mut child = parent.spawn((
                Mesh3d(skinned.unwrap_or(&part.mesh).clone()),
                MeshMaterial3d(init_mat),
                Transform::default(),
                ModelPart {
                    kind: ModelKind::Creature,
                    blend: part.blend,
                },
                // The picker's triangles (decision 0857): the `WOW_PICK` probe names worn gear
                // through `ModelPart`, and the render meshes are `RENDER_WORLD`-only.
                benilla_world::interact::PickMesh(part.geometry.clone()),
                // The portrait booth mirrors this rider ([`crate::portrait`]): steady
                // material (not the fade twin) + where it sits, so the bake can seat it at
                // the bone's bind-pose global (the booth spawns no skeleton). It stays the
                // STATIC mesh even for a rigged item — the booth has no palette to skin
                // through, and bind pose is what it wants to bake anyway.
                crate::portrait::PortraitRider {
                    static_mesh: part.mesh.clone(),
                    material: part.material.clone(),
                    bone,
                    offset,
                    attach: Some(hs.attach),
                },
            ));
            if skinned.is_some() {
                // The palette replaces this part's world matrix outright, so its model-local
                // `Aabb` below is not the volume it draws in — the same reason the effect lane's
                // skinned parts opt out (`spell_fx`). It also keeps the reference's own rule that
                // a scene object is elected ONCE, at the body root, never per batch (1473) — which
                // is what a rider's part would otherwise start doing, since the skinned twin keeps
                // main-world data and so earns the `calculate_bounds` the static twin never had.
                child.insert((
                    benilla_world::rig_palette::RigPart(root),
                    bevy::camera::visibility::NoFrustumCulling,
                ));
            }
            // Interior-light parity with the body: both material variants + a MeshTag, so the
            // classifier relights the weapon inside a WMO room like it does its wielder. While
            // joining the unit's appear-fade the tag carries its alpha instead — the classifier
            // yields via its own `Without<RenderFade>`/`Without<PendingAppearFade>` filter, same
            // as a body part.
            // Anchored at the WEARER's root: an equipped item M2 aliases its wearer's
            // light collector by pointer (`[item+0x3b8]=[wearer+0x3b8]`, `0x718960`), so it
            // never runs its own classify/footprint at the carried position. The animating hand
            // joint once anchored these, and a swing alone could trip the resample gate and
            // split the shield's light from the body's (director-caught, 2026-07-13).
            // The fold reference is the wearer's BODY centre for the same reason.
            //
            // The tag is unconditional (it used to ride the classifier's `Some`): it carries
            // the WEARER's instance slot, which is what puts a tinted body's colour on its
            // helm and shoulders — the director's report on the dwarf Stoneform tint. A part
            // with no interior variant got no tag at all before, so it was also invisible to
            // the ground-shade ramp that darkens its wielder; both now follow the body, which
            // is the same light-collector aliasing the comment above describes.
            child.insert(MeshTag(benilla_world::mesh_tag::spawn_tag(
                rig_slot, tag_alpha,
            )));
            // The item part's build-time bound (decision 0834): `calculate_bounds` can no longer
            // derive one from the `RENDER_WORLD`-only static form's data. Skipped for a skinned
            // part — it opted out of the frustum cull above, and a stale bound would only mislead
            // the picker volume that reads it.
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
            // The batch's authored **material alpha** — the combine's `colourAlpha × weight`
            // (`0x707680`). An attach model spawns no rig of its own and rests in its file's first
            // sequence, so its loops are PINNED there (the doodad lane's clock) while the tag
            // compose stays the unit lane's, ordered against the appear-fade and the interior
            // classifier exactly like the wearer's own batches.
            // Skipping this drew every one of the 321 item models that dim a batch at full
            // strength — the Hungering Cold's five glow cards blaze at 1.0 where the file says
            // 0.30 (decision 0836).
            if let Some(anim) = &part.alpha_anim {
                // `resting` is the attach model's pinned read — right for every held item but the
                // one that plays sequences (decision 2281): a flexing prop's per-sequence alpha
                // loops follow its own player, like a creature's do.
                child.insert(if flexes {
                    benilla_world::doodad_anim::MatAnim::following(anim.clone(), root)
                } else {
                    benilla_world::doodad_anim::MatAnim::resting(anim.clone())
                });
            }
            // …and the batch's texture transform (decision 2295), on the same one predicate every
            // other entity spawn asks. One held-item batch in the whole 1.12 corpus animates one —
            // `Item\ObjectComponents\Shield\shield_epic_a_01`, the Drillborer Disk, whose
            // Ragnaros-skinned lava is a 6.7 s global-sequence scroll — which is exactly why the
            // marker is a property of the batch here and not of a list somebody maintains.
            if part.uv_loops().animates() {
                child.insert(benilla_world::doodad_anim::AnimMatPart);
            }
            effective.dress(&mut child, &set);
        }
    });
    // The billboard cards (decision 0153): world-root entities FOLLOWING `root` — it sits
    // at the attach offset under the hand joint, is fresh per attach, and despawns on a
    // gear change, so the card's lifecycle and frame both come for free (same owner
    // contract as the item's emitters below).
    for (info, part) in billboard_parts {
        // …and, under `root`, the booth **mirror carrier** for that card (decision 0822).
        // The card itself is a world-ROOT entity, so the portrait / paper-doll booths — which
        // mirror the unit's dressed descendants — cannot see it; without this marker an
        // item's camera-facing batch (a wand's gem, this torch's `GLOWWHITE32` halo) simply
        // did not exist in those panes. Its seat is the attach point **plus the batch's own
        // model-local pivot**: an item model spawns no rig, so nothing else bakes that pivot.
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
        // A card is a batch of the item's model and joins the wearer's appear-fade exactly like
        // its mesh siblings above — same [`PartFade`], same seed, same arm. It used to spawn at
        // a flat opaque with neither `RenderFade` nor `FadeMaterials`, which is why a weapon's
        // glowing gems were already blazing before the character carrying them had faded in
        // (director-reported on the Hungering Cold; decision 0836).
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
            // The picker's triangles (decision 0857), pivot-centred by the caster like the bake.
            benilla_world::interact::PickMesh(part.geometry.clone()),
            BillboardCard::following(&info, root),
        ));
        // The card's build-time bound (decision 0834) — same rule as its mesh siblings above.
        if let Some(aabb) = part.aabb {
            card.insert(aabb);
        }
        // Same interior-light membership the item's mesh parts get above, through the same
        // constructor and anchored at the same WEARER (decision 0778) — so a held torch's
        // glow card can never split from the arm holding it. …and the wearer's instance slot,
        // like its mesh siblings: a tinted body colours the torch's glow card too (0812).
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
        // The card's own share of the authored material alpha (the Hungering Cold's gems are
        // keyed 0.30) — same pinned lane as the mesh parts.
        if let Some(anim) = &part.alpha_anim {
            card.insert(benilla_world::doodad_anim::MatAnim::resting(anim.clone()));
        }
        if part.uv_loops().animates() {
            card.insert(benilla_world::doodad_anim::AnimMatPart);
        }
        effective.dress(&mut card, &set);
    }
    // The item's own particle emitters — the held torch's flame (0130 phase 4: the same
    // owner-follow rider as doodad emitters). `root` sits at the attach offset under the
    // hand joint and its frame IS the item's model frame; a held item spawns no skeleton,
    // so the rest pose applies and no pivot rebase is needed — the flame burns at its
    // authored spot (the torch tip) and follows the hand through the swing. Free entities:
    // they self-despawn with `root` via the owner contract (gear change, unit despawn).
    // The spawn transform does two jobs, and neither is placing the emitter (the owner
    // overwrites the position every frame): its TRANSLATION seeds the flicker RNG —
    // root's entity bits de-sync two torch-bearers standing side by side — and its SCALE
    // is the wearer's, which is what the effects' draw-order rung is measured in
    // (`particles::owner_last_bias`). An item on a twice-size wielder reaches twice as
    // far from its own origin, so its rung has to grow with it; reading the scale off a
    // transform built as a bare RNG seed silently pinned every held effect at 1×.
    let spawn_tf = Transform::from_translation(Vec3::splat(root.to_bits() as f32))
        .with_scale(Vec3::splat(ctx.scale));
    // The booth mirror for those same emitters (decision 0822): they spawn as FREE entities
    // below (the owner contract), never unit descendants, so a booth that mirrors the dressed
    // tree cannot see them — which is why the R14 pauldron's sparkle was absent from the paper
    // doll exactly as it was from the select screen (`#bugs` B118). One marker per item, on
    // the root that already reaps with it.
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
        // …with ONE exception to "the rest pose applies": a **billboard** bone in the
        // emitter's chain. Its palette rows are replaced with the camera basis about its
        // own pivot every frame and children multiply onto that, so the reference's
        // emitter origin is `pivot + camBasis·(position − pivot)` — camera-dependent, and
        // up to two chain-offsets away from where the rest pose puts it (decision 0813).
        // The rig lane gets this from its joint palette; an item model has no rig, so the
        // frame is realized as a mesh-less billboard card the emitter OWNS-follows
        // (`BillboardCard::frame_following`). Nothing else in the chain is live: of the
        // 95 item models whose emitters ride a billboard bone, none animates its chain.
        //
        // …and ONE exception to *that*: a **flexing ranged prop** (decision 2281) does have a rig,
        // so its emitter rides its BONE's anchor. The firearm muzzle bank hangs off bone 2, which
        // BowRelease(161) keys a constant +90° about Y against Stand's 0° — on the root frame the
        // blast sprays out of the receiver, on the bone it goes down the barrel. `bone_pivot`
        // rebases the authored model-space origin into that bone's frame, exactly as the doodad and
        // widget lanes do. A billboard chain still wins the fork above: it is camera-replaced, and
        // no flexing model in the corpus has one.
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
                // A held item is an attached model — the flame fans with the swing.
                // The cloud anchors at the MODEL; the bone composes births only.
                anchor: Some(root),
                // The item model is destroyed when the item is replaced or unequipped, and the
                // reference frees a model's emitters at its dtor — so no cloud is left hanging in
                // the air behind the character (decision 0826). A sheath swap no longer comes
                // through here at all: the root is MOVED, and this pool rides it.
                on_owner_loss: benilla_world::particles::OwnerLoss::Free,
                // This emitter's own model instance is the item root; `ParentModel` above chains
                // it to the wearer, and the chain is what an ATTACHED model's composed alpha is
                // (`0x714000`) — so the sparkle on a pauldron fades in with the body wearing it
                // and vanishes with the avatar in first person (0827/0833).
                alpha: Some(root),
                // The light node is the WEARER's, never the item's: the reference has one node
                // per object and aliases the wearer's collector into each attached model
                // (`[item+0x3b8] = [wearer+0x3b8]`, `0x718960`) — the same rule this item's mesh
                // batches already classify under (`BodyBakeCenter`).
                light_node: Some(entity),
            },
            // A held item spawns no rig; its emitters run the item model's own **loader-idle**
            // slot on the spawn clock (the torch burns always — the doodad law). A flexing ranged
            // prop does have a player, and its rate/gate tracks read the sequence it is actually
            // on: the gun's blast is keyed to zero in Stand and to a 0.2 s burst in BowRelease,
            // so a pinned clock would emit nothing on it for ever (decision 2281).
            if flexes {
                benilla_world::particles::EmitClock::Host(root)
            } else {
                benilla_world::particles::EmitClock::Pinned
            },
        );
    }
    // The item's own M2 point light — **the held torch's glow** (decision 0016's law on the
    // entity half of the scene; see `super::carried_light`). `Club_1H_Torch_A_01.m2` — the
    // torch every torch-bearing NPC carries — authors exactly one: a warm
    // `(0.467, 0.290, 0.133) × 3.0` point light 0.58 yd up the shaft. It rides `root` like
    // the emitters and for the same reason (the item poses at rest, so its model space IS
    // the bone-local frame), which walks it through the hand's swing; the fence rails and
    // grass around the bearer then gather it like any other scene point light.
    spawn_carried_lights(commands, &dm.lights, root, |_| None);
    // The item's ribbon trails (weapon enchant streaks): ride the item root — a held item
    // poses at rest, so the bone-local origin is model-space (no pivot rebase). A held item
    // rests in Stand (anim 0): a thrown weapon's trail is keyed dark there, so the flight
    // ribbon never shows in the hand — it lights only on the InFlight missile.
    for rb in &dm.ribbons {
        benilla_world::ribbons::spawn_ribbon(
            commands,
            rb,
            root,
            false,
            ctx.scale,
            // A worn item rests in `Stand` and nothing on it plays anything else — the one lane
            // where the enable gate genuinely has a fixed answer for the instance's life.
            benilla_world::ribbons::RibbonSeq::Fixed(0),
            // Its own model instance — chained to the wearer above — so an enchant streamer is
            // gone with the avatar in first person and absent until the body is shown (0827/0833).
            Some(root),
            // No fade sphere: a carried item is not a placed model — it rides its wearer's
            // residency, and its streamer the wearer's render alpha one line up.
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

    /// One synthetic item part. `interior` picks whether it carries an interior material variant —
    /// the axis that used to decide whether the part got a `MeshTag` at all, and so whether it could
    /// ever be tinted or ground-shaded.
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

    /// A mesh handle distinguishable from the static form's `Handle::default()` — so "which of the
    /// two forms did the part actually spawn with" is an assertion rather than a guess.
    fn skinned_handle() -> Handle<Mesh> {
        Handle::Uuid(
            bevy::asset::uuid::Uuid::from_u128(0x5c1_11ed),
            std::marker::PhantomData,
        )
    }

    /// Spawn a wearer with one SHOULDER slot whose display optionally welds geometry to a billboard
    /// bone (decision 0841), and run the attach. Returns the part tags, the wearer's own instance
    /// slot, the item root's palette slot (`None` when it spawned no rig), and the mesh the part
    /// actually drew with.
    /// What the shoulder harness reports: the spawned parts' tags, the wearer's own slot, the
    /// item's slot (the one that is not the wearer's), the mesh it drew, and — the fact that
    /// separates 1609's rider from 0841's joint rig on the same shape — the [`RigRider`] the item
    /// root carries, if any.
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
        // The shoulder's own skeleton: a root plus one spherical-billboard spike, the shape
        // `LShoulder_Plate_PVPAlliance_A_01` authors. Both halves are needed — the gate is
        // "welded geometry AND a skeleton to pose it with".
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
        // The item root's rig is the one that is NOT the wearer's (the wearer holds its own).
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

    /// The welded case, named for the assertion that reads it.
    fn attach_a_welded_shoulder() -> Attached {
        attach_a_shoulder(true)
    }

    /// Spawn a wearer with one helm slot and run the attach. Returns the spawned parts' tags plus the
    /// wearer's own instance slot (`0` when `rigged` is false — a boneless wearer).
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

    /// **The director's report (dwarf Stoneform tinted the body but not the helm/shoulders).** An
    /// attachment carries its WEARER's instance slot, so the body tint reaches it — decision 0812's
    /// named gap, closed. The reference's own rule: an attached model inherits the parent CM2's
    /// computed colours (`0x714000`).
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

    /// The tag is unconditional now. It used to ride the interior classifier's `Some`, so a part with
    /// no interior variant carried none at all — invisible to the tint AND to the ground-shade ramp
    /// that darkens its wielder. Both halves of that are asserted: a tag exists, and it is opaque.
    #[test]
    fn a_part_without_an_interior_variant_still_gets_a_tag() {
        let (tags, _) = attach_a_helm(true, false);
        assert_eq!(tags.len(), 1);
        assert_ne!(tags[0], 0, "not the untagged ⇒ opaque sentinel");
        assert!((benilla_world::mesh_tag::alpha_of(tags[0]) - 1.0).abs() < 1.0 / 63.0);
    }

    /// A wearer with no rig at all (a boneless model holding something) lands on the identity slot 0
    /// rather than borrowing someone else's colour.
    #[test]
    fn a_rigless_wearer_leaves_the_slot_at_identity() {
        let (tags, _) = attach_a_helm(false, true);
        assert_eq!(tags.len(), 1);
        assert_eq!(benilla_world::mesh_tag::rig_of(tags[0]), 0);
    }

    /// **The item rig (decision 0841, restored by 0854).** A display whose geometry is welded to a
    /// billboard bone has no correct rigid placement — 0839 stopped tearing it into a card, leaving
    /// it whole and still. Such an item spawns a rig of its own, and the three things that makes
    /// true are asserted together because any one of them alone is a silent no-op: the part draws
    /// the **skinned** twin (a static mesh is never skinned — `WOW_RIG_SKIN` compiles off the vertex
    /// layout, not the tag), it carries its OWN palette slot rather than the wearer's, and the root
    /// really holds the `RigSkin` that slot belongs to.
    ///
    /// 0847 withdrew this and pinned the withdrawal with the mirror of this test; both are gone
    /// again. The withdrawal's premise — a spherical billboard sweeping the spikes through the
    /// plate — was refuted at the geometry (0853) and at the bytes (`0x718657`–`0x71876f`), and
    /// [`benilla_world::billboard::tests::a_spike_along_its_bone_axis_points_screen_down_from_every_angle`]
    /// is the standing guard that the arc stays absent in our own basis.
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
        // The welded lane keeps its own camera-replaced JOINT rig — it is not a rigid frame, and
        // that is exactly why 1609's rider cannot serve it (see `rig_rider`'s residuals).
        assert_eq!(a.joints, 1, "the welded item runs a billboard joint rig");
        assert!(a.rider.is_none(), "…and is not a rider");
    }

    /// **The rider (decision 1609).** The counter-anchor, on the same harness: an ordinary item —
    /// the 9684 models that weld nothing — now takes a palette slot too, but a different KIND of
    /// one. It is one rigid frame composed in the wearer's rig frame, not a joint rig, and the
    /// four things that makes true are asserted together because any one alone is a silent no-op:
    /// the part draws the **skinned** twin (a static mesh is never skinned), it carries its OWN
    /// slot rather than the wearer's (the vertex stage indexes the palette with that field), the
    /// root holds a [`RigRider`] naming the wearer and the attach bone, and no joint rig was built.
    ///
    /// Until 1609 this test asserted the opposite — no slot, the wearer's tag, the static mesh —
    /// because the ordinary lane placed an item through its entity's absolute `GlobalTransform`.
    /// At Elwynn's ~9481 yards that sum lands on an 0.98 mm f32 grid and is rebuilt every frame,
    /// so a standing guard's helm hopped a whole grid step every other frame while the body under
    /// it moved exactly. The static-mesh route is what this test now forbids coming back.
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

    /// **The ranged prop gets a clock, and only the ranged prop** (decision 2281).
    ///
    /// Four things have to be true together or the gun's muzzle blast never fires, and each one
    /// alone is a silent no-op: the prop root carries a [`crate::ranged_flex::RangedProp`] naming
    /// its wearer (that is how a `$BWR` key finds it), a [`benilla_world::rig_anim::RigPose`] and a
    /// player (so 161 can be armed and posed at all), its emitter is **hosted on that root** rather
    /// than pinned to the loader idle (the blast's gate track is keyed only in 161 — a pinned
    /// emitter reads Stand's flat zero for ever), and the emitter rides its BONE's anchor rather
    /// than the model root (bone 2 is what aims it down the barrel).
    ///
    /// The control is the same display in the MAINHAND slot: nothing is ever passed to the
    /// reference's `[+0xd24]` arms but the ranged weapon, so a melee item keeps the resting lane —
    /// no player, no pose, a pinned clock.
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
                    // The muzzle bone the emitters hang off.
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
                // `owns(161)` — the gate, the reference's own `0x711960` question.
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

        // The ranged slot (2), drawn in the right hand.
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
        // The emitter's owner frame is the muzzle bone's anchor, not the model root.
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

        // The control: the SAME display in the mainhand keeps the resting lane.
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

    /// **What a booth can see of an equipped item** (decision 0822, `#bugs` B118's paper-doll half).
    /// An item model's camera-facing batch spawns as a world-ROOT card and its emitters as free
    /// owner-followed entities — neither is a unit descendant, so the portrait / paper-doll booths,
    /// which mirror the dressed tree, could not see either one and a worn item's effects were absent
    /// from every pane. The attach must therefore publish a mirror for both, at the seat the booth
    /// needs: the attach point **plus the batch's own model-local pivot** for a card (an item spawns no
    /// rig, so nothing else bakes that pivot), the bare attach point for the effect host.
    ///
    /// Both offsets are asserted against a nonzero attach point AND a nonzero pivot, so publishing
    /// either one alone — or adding them in the wrong frame — fails.
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
        // The R14 pauldron's shape: a plain mesh batch, a camera-facing batch, and emitters.
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

    /// **The director's report on the Hungering Cold** (decision 0836): a weapon's glowing gems
    /// were already blazing at full strength before the character carrying them had faded in.
    /// The sword authors five camera-facing `GENERICGLOW1` batches, and a card used to spawn with
    /// neither `RenderFade` nor `FadeMaterials` — a batch that pops. It is a batch of the item's
    /// model like any other, so it joins the wearer's ramp with the mesh parts.
    ///
    /// The same spawn also pins the batch's **authored** alpha: the sword's cards are keyed
    /// `weight 0.30` and every item batch drew at 1.0, because no attach-model part ever carried a
    /// `MatAnim` at all (321 of the 2681 shipped item models dim a batch this way).
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
        // The wearer is mid-login: its own ramp is still pending, so everything attaching to it
        // must join that ramp rather than open opaque beside it.
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

    /// **The director's login report on the Naxx items** (decision 0865): a MULTIPLY sheen batch
    /// (Mod2x — the ARMORREFLECT family) used to spawn Steady, popping as a full-strength ×2 layer
    /// over a body still fading in. Its blend equation reads no alpha, so no material swap can
    /// feather it (0528) — instead the part arms the ramp on its STEADY material (the "twin" is
    /// itself) and the shader lerps its colour toward the blend identity by the tag alpha, which
    /// is the reference's own preset-5 fade (1489; 0865 built it believing it a deviation).
    /// Headless, this asserts the arm half: joined ramp, tag alpha ≈ 0, no material swap, and a
    /// FadeMaterials record so despawn/stealth ramps re-arm it too.
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
        // What `entities::display` now builds for a multiply batch: the steady self as the twin.
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

    /// A wearer holding a **weapon** (mainhand, drawn in the hand) and wearing a **shoulder** that
    /// carries emitters — the bench for the sheath-swap tests below. Returns the app and the wearer.
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
            // A real item model has a skeleton and a skinned twin, so it takes a rider slot
            // (1609) — which is what makes the sheath swap's rider re-seat below observable.
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

        // Three attach points on three bones: the hand, the back (where a stowed weapon rides)
        // and the shoulder — their joints spawn on first demand (decision 1355).
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
    /// A back sheath point (`attach_id`'s `BACK_SHEATH` family — any id the body publishes).
    const SHEATH_BACK: u16 = 6;

    /// The roots this wearer currently has attached, per slot.
    fn roots_of(app: &App, wearer: Entity) -> [Option<Entity>; ATTACH_SLOTS] {
        app.world()
            .entity(wearer)
            .get::<HeldAttached>()
            .unwrap()
            .spawned
    }

    /// **The director's report, at its cause** (decision 0826): drawing/stowing a weapon changed one
    /// slot's attach point, and the old code rebuilt the unit's WHOLE kit — so the shoulders' and
    /// helm's emitters were orphaned mid-swing and their live particles hung in world space while
    /// the character walked on ("armor and weapon particles … lag behind when doing a weapon draw").
    ///
    /// Three things are asserted, and each one alone would reproduce the bug if it regressed: the
    /// untouched slot keeps its root entity (so its pool is never orphaned), the moved slot keeps
    /// ITS root (so the weapon's own effects ride the swap, as the reference's re-parent does), and
    /// the moved root really is re-seated — new parent joint, new local offset.
    #[test]
    fn a_sheath_swap_moves_the_weapon_and_leaves_the_other_slots_alone() {
        let (mut app, wearer) = dress_a_wearer();
        let before = roots_of(&app, wearer);
        let (weapon, shoulder_root) = (before[0].expect("weapon"), before[4].expect("shoulder"));

        // Stow it: the same item, a new attach point — nothing else about the kit changes.
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
        // The booth mirrors move with it — a paper doll must show the stowed weapon on the back.
        let seat = app
            .world_mut()
            .query::<&crate::portrait::PortraitRider>()
            .iter(app.world())
            .find(|r| r.offset.distance(BACK_AT) < 1e-5)
            .map(|r| r.bone);
        assert_eq!(seat, Some(2), "the rider's cached seat followed the move");
        // **And the palette rider followed it too** (decision 1609). The MOVE is the one edit that
        // keeps the item's entity alive while changing the bone its frame is composed from, so the
        // rider is the one thing a re-parent can silently leave behind: the sword would go on
        // drawing at the hand it was stowed from while its entity, its glow and its emitters all
        // travelled to the back.
        let rider = app
            .world()
            .entity(weapon)
            .get::<benilla_world::rig_rider::RigRider>()
            .expect("the weapon rides a rider frame");
        assert_eq!(rider.bone, 2, "the rider is composed from the sheath bone");
        assert_eq!(rider.local, BACK_AT, "…at the sheath point's offset");
        assert_eq!(rider.host, wearer);
    }

    /// The other half of the per-slot diff: a slot whose item genuinely CHANGED is rebuilt (its old
    /// model is destroyed — the reference's dtor, which takes that model's emitters with it), and
    /// still nobody else's slot is touched.
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

    /// The chain's first link, on the real spawn path (decision 0833): an item model is CHAINED to
    /// the body wearing it, and the emitters it spawns point at **their own** root rather than at
    /// the wearer. Both halves matter — the item's own sparkle would fade correctly either way,
    /// but a glow instance hung on this root two links down can only reach the wearer through it,
    /// and that is the link the enchant-glow lane never had.
    ///
    /// It survives the sheath MOVE for the same reason the pool does: the root is re-parented, not
    /// rebuilt.
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
