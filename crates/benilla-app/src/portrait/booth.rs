//! The booth bake spawn: a mirrored look becomes the posed throwaway instance the camera shoots
//! (the reference's instance `0x707400`, pose `0x7121a0`). The skeleton is one
//! [`RigPose`](benilla_world::rig_anim::RigPose) buffer on the booth root; riders, cards and
//! effect hosts seat on demand-spawned anchors ([`BoothRig::anchor`]).

use benilla_formats::BillboardKind;
use bevy::animation::RepeatAnimation;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;
use benilla_world::model_fade::FadeMaterials;

/// One mesh headed into a booth bake: the mirrored part's twins and its studio-lit material.
pub(super) struct BoothPart {
    pub(super) skinned: Option<Handle<Mesh>>,
    pub(super) static_mesh: Handle<Mesh>,
    pub(super) material: Handle<WowModelMaterial>,
    /// The batch's animated material alpha, often an authored dimming constant (UI_Tauren's 0.55
    /// `LENSALPHA` vignette); `None` for an opaque batch.
    pub(super) alpha_anim: Option<std::sync::Arc<benilla_formats::AlphaAnim>>,
    /// The blend and depth-prime twins a batch draws through when the instance alpha is below 1;
    /// `None` for a batch the reference's twin gate says cannot feather.
    pub(super) twins: BoothTwins,
    /// Whether the builder registered this batch's material on the UV and tint lane. It records the
    /// registration, never re-tests the asset: the marker and the registration must be one fact.
    /// True only for the glue scenes, which animate texture transforms per frame (`0x715f25` to
    /// `0x7163bc` over `md+0x74`); portraits and panes are stills.
    pub(super) mat_anim: bool,
}

/// Marks a booth child for the draw gate when its material is on the UV and tint lane; an
/// unmarked registered batch stays frozen at its seed.
fn mark_mat_anim(child: &mut bevy::ecs::system::EntityCommands<'_>, p: &BoothPart) {
    if p.mat_anim {
        child.insert(benilla_world::doodad_anim::AnimMatPart);
    }
}

/// A batch's translucency twins; the default, both `None`, cannot feather.
#[derive(Clone, Default)]
pub(crate) struct BoothTwins {
    pub(crate) blend: Option<Handle<WowModelMaterial>>,
    pub(crate) zfill: Option<Handle<WowModelMaterial>>,
}

/// The bake's instance-level render properties: the reference's per-instance alpha
/// (`model+0x180`), which every batch draws through and every attached model composes onto
/// (`0x714000`), so an item on a ghost is as translucent as the arm holding it. The tag alpha, the
/// blend-twin swap and the [`FadeMaterials`] record are applied together in [`spawn_booth_model`].
#[derive(Clone, Copy)]
pub(super) struct BoothInstance {
    /// `model+0x180`; 1.0 is opaque, every bake but a ghosted one.
    pub(super) alpha: f32,
}

impl Default for BoothInstance {
    fn default() -> Self {
        Self { alpha: 1.0 }
    }
}

impl BoothInstance {
    /// Whether this instance draws translucent, the reference's `0 < A < 1` band
    /// ([`benilla_world::mesh_tag::translucent`]).
    pub(super) fn feathering(self) -> bool {
        self.alpha < 1.0
    }

    /// The blend twin while feathering, else `steady`. A Mod/Mod2x batch has no twin and reads no
    /// alpha; it fades through the shader's identity lerp on the tag alpha instead.
    fn material(
        self,
        steady: &Handle<WowModelMaterial>,
        twins: &BoothTwins,
    ) -> Handle<WowModelMaterial> {
        match (self.feathering(), twins.blend.as_ref()) {
            (true, Some(blend)) => blend.clone(),
            _ => steady.clone(),
        }
    }

    /// The [`FadeMaterials`] record a feathering batch carries, so [`benilla_world::zfill`]'s depth
    /// prime arms on it as on a stealthed body in the world.
    fn fade_materials(
        self,
        steady: &Handle<WowModelMaterial>,
        twins: &BoothTwins,
    ) -> Option<FadeMaterials> {
        let blend = twins.blend.clone()?;
        self.feathering().then(|| FadeMaterials {
            cutout: steady.clone(),
            blend,
            // A booth is never interior-classified: its light is the scene's own rig.
            bake_blend: None,
            zfill: twins.zfill.clone(),
        })
    }
}

/// Marks a booth part whose render-alpha `MeshTag` and `Visibility` its own
/// [`MatAnim`](benilla_world::doodad_anim::MatAnim) drives ([`push_booth_mat_alpha`]). The world's
/// visibility writer culls against the world camera, so a booth part stays out of it and this
/// marker claims sole `Visibility` authority, including the reference's `A <= 0` cull, which
/// fires before the blend mode is read (`0x707b3a` to `0x707b5c`).
#[derive(Component)]
pub(super) struct BoothMatAlpha;

/// One bone rider headed into a booth bake ([`PortraitRider`], studio-lit).
pub(super) struct BoothRider {
    pub(super) mesh: Handle<Mesh>,
    pub(super) material: Handle<WowModelMaterial>,
    pub(super) bone: u16,
    pub(super) offset: Vec3,
    /// An attached model composes onto its parent's alpha (`0x714000`), so a ghost's helm
    /// feathers with the head under it.
    pub(super) twins: BoothTwins,
}

/// One camera-facing batch headed into a booth bake: the undead or night-elf eye glow, or an
/// equipped item's own such batch. The booth seats the centred quad under its bone's joint and
/// faces it to the booth camera ([`face_booth_billboards`]).
pub(super) struct BoothBillboardSpec {
    pub(super) mesh: Handle<Mesh>,
    pub(super) material: Handle<WowModelMaterial>,
    pub(super) bone: u16,
    /// The card pivot's offset under the bone's joint (Bevy axes); `ZERO` for a batch of the
    /// rigged model itself, whose joint already holds the pivot.
    pub(super) offset: Vec3,
    pub(super) kind: BillboardKind,
    /// A card takes the instance alpha like every other batch of the model.
    pub(super) twins: BoothTwins,
}

/// A spawned booth billboard card, faced to its own booth's camera every frame by
/// [`face_booth_billboards`].
#[derive(Component)]
pub(super) struct BoothBillboard {
    kind: BillboardKind,
}

impl BoothBillboard {
    /// A mesh-less billboard frame: its world rotation is the billboard bone's replaced basis
    /// about the booth camera, for an emitter under that bone to ride; the reference folds the
    /// emitter's position through this matrix (`0x7190a9` to `0x71910c`, in `0x718960`). The
    /// caller owns the translation; [`face_booth_billboards`] writes only the rotation.
    pub(super) fn frame(kind: BillboardKind) -> Self {
        Self { kind }
    }
}

/// One effect-bearing model headed into a booth bake (an equipped item's emitters or an
/// `ItemVisuals` glow): its emitters, the body bone it rides and its offset in that bone's frame.
/// Its geometry rides the [`BoothRider`] and [`BoothBillboardSpec`] lists at the same seat.
pub(super) struct BoothEffects {
    pub(super) bone: u16,
    pub(super) offset: Vec3,
    pub(super) emitters: Vec<benilla_assets::ModelEmitter>,
}

/// The in-flight bake's rig: the booth root and its pending [`RigPose`] buffer (`None` for a
/// boneless bake). The caller must [`Self::finish`] once every consumer is seated: the anchors
/// live in the buffer, which reaches the ECS only then.
#[must_use = "call finish(): the pose buffer reaches the booth root only then"]
pub(super) struct BoothRig {
    root: Entity,
    rig: Option<benilla_world::rig_anim::RigPose>,
    /// The palette slot this bake allocated (0 for a boneless bake or a full pool), also its index
    /// into the per-instance tint table ([`benilla_world::instance_tint`]).
    slot: u16,
}

impl BoothRig {
    /// The anchor entity for `bone`, spawned on first demand and re-seated by the compose pass
    /// every animated frame; `None` for a boneless bake or a bone outside the skeleton.
    pub(super) fn anchor(&mut self, commands: &mut Commands, bone: u16) -> Option<Entity> {
        let root = self.root;
        self.rig.as_mut()?.anchor_for(commands, root, bone)
    }

    /// Whether a rig stands at all, the park gate's test.
    pub(super) fn rigged(&self) -> bool {
        self.rig.is_some()
    }

    /// This bake's palette and tint slot; 0 is the no-rig sentinel, so the bake cannot be tinted.
    pub(super) fn slot(&self) -> u16 {
        self.slot
    }

    /// Commits the pose buffer onto the booth root with `StageRig`, together, so the world-view
    /// parker never sees a booth `RigPose` without the marker that exempts it.
    pub(super) fn finish(self, commands: &mut Commands) {
        if let Some(rig) = self.rig {
            commands.entity(self.root).insert((rig, super::StageRig));
        }
    }
}

/// Strips a booth root's rig state. Every empty or re-bake arm must run it: the child despawn
/// reaps meshes and anchors, but the pose buffer, player, palette slot and park marker live on the
/// root and would keep evaluating and writing palette rows.
pub(super) fn clear_booth_rig(commands: &mut Commands, root: Entity) {
    commands.entity(root).remove::<(
        AnimationPlayer,
        AnimationGraphHandle,
        benilla_assets::ModelAnimations,
        benilla_world::rig_anim::GlobalSeqDrive,
        benilla_world::rig_anim::RigPose,
        benilla_world::rig_anim::AnimParked,
        super::StageRig,
        benilla_world::rig_palette::RigSkin,
    )>();
}

/// Spawns `effects` into a bake, one host per effect model on its bone's anchor, and returns
/// `(emitters, billboard frames)`. `light` is the booth's own light buffer, so the world's time of
/// day never shades a pane.
///
/// Only live booths call this. A `<PlayerModel>` pane renders every frame (`0x76d680`), while
/// `SetPortraitTexture` bakes a round portrait in one draw and caches it (`0x524f60`), where a
/// newborn particle pool draws nothing.
pub(super) fn spawn_booth_effects(
    commands: &mut Commands,
    rig: &mut BoothRig,
    layer: &RenderLayers,
    light: Option<&bevy::render::render_resource::Buffer>,
    effects: &[BoothEffects],
    // An effect model attached to a translucent instance composes onto its alpha.
    instance: BoothInstance,
) -> (usize, usize) {
    let mut spawned = 0usize;
    let mut frames = 0usize;
    for fx in effects {
        let Some(joint) = rig.anchor(commands, fx.bone) else {
            continue; // bad bone index: bake the body without this model's effects
        };
        // The host stands for the attached model: the reference composes its alpha every frame as
        // `child+0x19c = parent+0x19c × child+0x180` (`0x714260`) and folds it into every particle
        // through `emitter+0x1a8` (`0x718960` at `0x719073`); `ModelAlphas` walks that chain.
        // `ModelFade`, not `ModelAlpha`: a bake has no ramps, so the declared value is read as is.
        let mut host = commands.spawn((
            Transform::from_translation(fx.offset),
            Visibility::default(),
            ChildOf(joint),
            layer.clone(),
        ));
        if instance.feathering() {
            host.insert(benilla_world::model_fade::ModelFade(instance.alpha));
        }
        let host = host.id();
        for em in &fx.emitters {
            // A billboard bone in the chain puts the origin at
            // `pivot + camBasis·(position − pivot)`, so a mesh-less `BoothBillboard` at the pivot
            // carries the booth camera's basis. With none (every shipped glow model, most items)
            // the host owns the emitter.
            let owner = match em.billboard {
                Some(bb) => {
                    let frame = commands
                        .spawn((
                            Transform::from_translation(benilla_assets::coords::wow_to_bevy(
                                bb.pivot,
                            )),
                            layer.clone(),
                            ChildOf(host),
                            BoothBillboard::frame(bb.kind),
                        ))
                        .id();
                    frames += 1;
                    (frame, bb.pivot)
                }
                None => (host, [0.0; 3]),
            };
            let Some(e) = benilla_world::particles::spawn_emitter(
                commands,
                em,
                Transform::IDENTITY,
                benilla_world::particles::EmitterFrames {
                    owner: Some(owner),
                    anchor: Some(host), // the cloud anchors at the model; bones compose births only
                    // A booth rider's host is torn down with the bake it belongs to.
                    on_owner_loss: benilla_world::particles::OwnerLoss::Free,
                    // The host carries the bake's instance alpha; an opaque bake reads 1.0.
                    alpha: Some(host),
                    // The booth's own light buffer is bound below, never a world light node.
                    light_node: None,
                },
                benilla_world::particles::EmitClock::Pinned, // an item's effects loop forever
            ) else {
                continue;
            };
            commands.entity(e).insert((layer.clone(), ChildOf(host)));
            if let Some(buf) = light {
                commands
                    .entity(e)
                    .insert(benilla_world::particles::buffer::EffectLightOverride(
                        buf.clone(),
                    ));
            }
            spawned += 1;
        }
    }
    (spawned, frames)
}

/// Spawns a booth model's own emitters, on its own joints (a scene's braziers, a pet's flames),
/// and returns `(emitters, billboard frames)`. `root` is the fallback owner, the cloud's anchor
/// and every emitter's parent, so a re-bake reaps them.
///
/// A booth pose drops the camera billboards
/// ([`benilla_world::rig_anim::RigPose::without_camera_billboards`]), so an emitter under a
/// billboard bone rides a [`BoothBillboard`] frame on that bone's joint at `ZERO`, or at the
/// pivot under `root` when the bone has no anchor.
pub(super) fn spawn_booth_own_emitters(
    commands: &mut Commands,
    rig: &mut BoothRig,
    root: Entity,
    layer: &RenderLayers,
    light: Option<&bevy::render::render_resource::Buffer>,
    emitters: &[benilla_assets::ModelEmitter],
) -> (usize, usize) {
    let mut spawned = 0usize;
    let mut frames = 0usize;
    for em in emitters {
        let owner = match em.billboard {
            Some(bb) => {
                // The joint holds the pivot, so the frame sits at ZERO; with no joint, the
                // rest-pose placement at the pivot under the root.
                let (seat, at) = match rig.anchor(commands, bb.bone) {
                    Some(joint) => (joint, Vec3::ZERO),
                    None => (root, benilla_assets::coords::wow_to_bevy(bb.pivot)),
                };
                let frame = commands
                    .spawn((
                        Transform::from_translation(at),
                        layer.clone(),
                        ChildOf(seat),
                        BoothBillboard::frame(bb.kind),
                    ))
                    .id();
                frames += 1;
                (frame, bb.pivot)
            }
            // The emitter rides its own bone's joint, in that bone's frame.
            None => rig
                .anchor(commands, em.def.bone)
                .map_or((root, [0.0; 3]), |joint| (joint, em.bone_pivot)),
        };
        let Some(e) = benilla_world::particles::spawn_emitter(
            commands,
            em,
            Transform::IDENTITY,
            benilla_world::particles::EmitterFrames {
                owner: Some(owner),
                anchor: Some(root),
                // The booth's own light buffer is bound below, never a world light node.
                light_node: None,
                // The bake root is torn down and rebuilt as a whole.
                on_owner_loss: benilla_world::particles::OwnerLoss::Free,
                // A booth bake has no appear or despawn ramp and no self-avatar feather.
                alpha: None,
            },
            // A booth loops its one authored clip forever, as a doodad does.
            benilla_world::particles::EmitClock::Pinned,
        ) else {
            continue;
        };
        commands.entity(e).insert((layer.clone(), ChildOf(root)));
        if let Some(buf) = light {
            commands
                .entity(e)
                .insert(benilla_world::particles::buffer::EffectLightOverride(
                    buf.clone(),
                ));
        }
        spawned += 1;
    }
    (spawned, frames)
}

/// How the bake's Stand runs: `Frozen` at t = 0 for a portrait still (the reference's bake is a
/// still; its sampling time is untraced), `Loop` for the live glue scenes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BoothMotion {
    Frozen,
    Loop,
}

/// Spawns a booth bake under `root`: a fresh throwaway instance posed at Stand (the reference's
/// instance `0x707400`, pose `0x7121a0`), never the unit's live pose.
///
/// With a rig, skinned parts draw against a palette slot, riders seat on bone anchors and Stand
/// (anim 0) is armed frozen or looping per `motion`; the reference's bake sampling clock is
/// untraced, and t = 0 is a valid Stand frame. Without a rig, parts bake at bind pose and riders
/// are dropped. Seat any remaining consumers on the returned [`BoothRig`], then `finish()` it.
pub(super) fn spawn_booth_model(
    commands: &mut Commands,
    palettes: &mut benilla_world::rig_palette::RigPalettes,
    root: Entity,
    layer: RenderLayers,
    parts: &[BoothPart],
    riders: &[BoothRider],
    rig: Option<(
        &benilla_assets::ModelSkeleton,
        &Handle<bevy::mesh::skinning::SkinnedMeshInverseBindposes>,
        Option<&benilla_assets::ModelAnimations>,
    )>,
    catalog: Option<&benilla_formats::AnimDataCatalog>,
    motion: BoothMotion,
    // Per-hand weapon grip `[right, left]`: a hand whose attach point holds a weapon keeps its
    // `HandsClosed` pose (`0x5059a0`). Round portraits pass `[false, false]`: the reference's
    // portrait bake arms no grip.
    grip: [bool; 2],
    // Character billboard batches (the eye glow); the boneless bake drops them.
    billboards: &[BoothBillboardSpec],
    // Opaque by default; translucent only for a ghosted character select.
    instance: BoothInstance,
) -> BoothRig {
    // A re-bake must not inherit the previous model's animation state, pose buffer or palette slot.
    clear_booth_rig(commands, root);
    let Some((skeleton, ibp, anims)) = rig.filter(|(s, _, _)| !s.joints.is_empty()) else {
        for p in parts {
            let mut child = commands.spawn((
                Mesh3d(p.static_mesh.clone()),
                MeshMaterial3d(instance.material(&p.material, &p.twins)),
                Transform::IDENTITY,
                layer.clone(),
                ChildOf(root),
            ));
            mark_mat_anim(&mut child, p);
            // The batch's material alpha times the instance alpha, the reference's
            // `model+0x180 × the batch factor` (`0x707680`).
            if let Some(anim) = &p.alpha_anim {
                let mat_anim =
                    benilla_world::doodad_anim::MatAnim::driving_tag(anim.clone(), 0.0, None);
                child.insert((
                    bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(
                        0,
                        mat_anim.current * instance.alpha,
                    )),
                    mat_anim,
                    BoothMatAlpha,
                ));
            } else if instance.feathering() {
                child.insert(bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(
                    0,
                    instance.alpha,
                )));
            }
            if let Some(fm) = instance.fade_materials(&p.material, &p.twins) {
                child.insert(fm);
            }
        }
        return BoothRig {
            root,
            rig: None,
            slot: 0,
        };
    };
    // The pose buffer, bind-pose composed; its camera billboards are dropped because the booth's
    // cards face their own camera.
    let mut pose =
        benilla_world::rig_anim::RigPose::new(root, skeleton).without_camera_billboards();
    // The palette rig the skinned parts tag; the booth's studio light buffer mirrors its rows
    // (`rig_palette::RigPaletteMirrors`), so the booth camera sees the pose.
    let rig_slot = benilla_world::rig_palette::RigSkin::allocate_bones(
        palettes,
        skeleton.joints.len() as u32,
        ibp.clone(),
    )
    .map_or(0, |rig| {
        let slot = rig.slot;
        commands.entity(root).insert(rig);
        // Booth materials bind a studio light buffer, so route this rig's rows to the mirrors.
        palettes.mark_mirrored(slot);
        slot
    });
    // Global-sequence bones: `Loop` runs them on the world's clock (fires flicker, the windmill
    // turns, the character blinks); `Frozen` samples t = 0, where the eyelid's scale is 0 and the
    // eye open. Stand keys none of them, so the paused player never overwrites the freeze.
    if let Some(anims) = anims {
        match motion {
            BoothMotion::Loop => {
                if let Some(drive) = benilla_world::rig_anim::GlobalSeqDrive::new_rig(
                    &anims.global_bones,
                    pose.locals.len(),
                ) {
                    commands.entity(root).insert(drive);
                }
            }
            BoothMotion::Frozen => {
                for gb in &anims.global_bones {
                    // `locals` are seeded at bind pose, so untouched properties keep the rest
                    // transform.
                    let Some(tf) = pose.locals.get_mut(gb.bone as usize) else {
                        continue;
                    };
                    if let Some(c) = &gb.translation {
                        tf.translation = c.sample(0.0);
                    }
                    if let Some(c) = &gb.rotation {
                        tf.rotation = c.sample(0.0);
                    }
                    if let Some(c) = &gb.scale {
                        tf.scale = c.sample(0.0);
                    }
                }
            }
        }
    }
    for p in parts {
        let use_rig = p.skinned.is_some() && rig_slot != 0;
        let mesh = if use_rig {
            p.skinned.clone().expect("use_rig ⇒ skinned twin present")
        } else {
            p.static_mesh.clone()
        };
        let mut child = commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(instance.material(&p.material, &p.twins)),
            Transform::IDENTITY,
            layer.clone(),
            ChildOf(root),
        ));
        mark_mat_anim(&mut child, p);
        // The material alpha drives the tag's alpha field itself (nothing else writes a booth
        // part's alpha) beside the rig slot. `alpha_bits` floors a true zero just above 0, so the
        // shader's all-zero "untagged, opaque" sentinel cannot fire on an invisible batch.
        let tag_slot = if use_rig { rig_slot } else { 0 };
        if let Some(anim) = &p.alpha_anim {
            let mat_anim =
                benilla_world::doodad_anim::MatAnim::driving_tag(anim.clone(), 0.0, None);
            child.insert((
                bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(
                    tag_slot,
                    mat_anim.current * instance.alpha,
                )),
                mat_anim,
                BoothMatAlpha,
            ));
        } else if use_rig || instance.feathering() {
            child.insert(bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(
                tag_slot,
                instance.alpha,
            )));
        }
        // While translucent, `benilla_world::zfill` keeps a colour-masked z-writing child on it, so
        // overlapping limbs blend as one layer (the reference's `M2UseZFill`).
        if let Some(fm) = instance.fade_materials(&p.material, &p.twins) {
            child.insert(fm);
        }
        if use_rig {
            child.insert(benilla_world::rig_palette::RigPart(root));
            // A skinned booth part is never frustum-culled: Bevy's bound is the bind-pose box, the
            // camera frames the Stand pose, and on a model whose rest pose is far from Stand
            // (`Creature\CarrionBird`, bind z to 1.19, Stand z 0.54 to 6.23) every batch culls.
            child.insert(bevy::camera::visibility::NoFrustumCulling);
        }
    }
    let mut booth_rig = BoothRig {
        root,
        rig: Some(pose),
        slot: rig_slot,
    };
    for r in riders {
        let Some(anchor) = booth_rig.anchor(commands, r.bone) else {
            continue; // bad bone index: bake the body without this rider
        };
        let mut child = commands.spawn((
            Mesh3d(r.mesh.clone()),
            MeshMaterial3d(instance.material(&r.material, &r.twins)),
            Transform::from_translation(r.offset),
            layer.clone(),
            ChildOf(anchor),
        ));
        // An attached model composes onto its parent's colour and alpha (`0x714000`). Untagged
        // means opaque and slot 0, so the tag goes up only for a translucent instance or a slot.
        if instance.feathering() || rig_slot != 0 {
            child.insert(bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(
                rig_slot,
                instance.alpha,
            )));
        }
        if let Some(fm) = instance.fade_materials(&r.material, &r.twins) {
            child.insert(fm);
        }
    }
    // Camera-facing batches seat under their bone's anchor for `face_booth_billboards` to face;
    // a bone the rig lacks drops the card, like a rider.
    for bb in billboards {
        let Some(anchor) = booth_rig.anchor(commands, bb.bone) else {
            continue;
        };
        let mut card = commands.spawn((
            Mesh3d(bb.mesh.clone()),
            MeshMaterial3d(instance.material(&bb.material, &bb.twins)),
            Transform::from_translation(bb.offset),
            layer.clone(),
            ChildOf(anchor),
            BoothBillboard { kind: bb.kind },
        ));
        if instance.feathering() || rig_slot != 0 {
            card.insert(bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(
                rig_slot,
                instance.alpha,
            )));
        }
        if let Some(fm) = instance.fade_materials(&bb.material, &bb.twins) {
            card.insert(fm);
        }
    }
    // The player is configured before insertion, so the pose lands with the first animation pass;
    // `ModelAnimations` goes beside it because the rig evaluator reads the player through it.
    if let Some(anims) = anims {
        let stand = catalog.map_or(0, |c| anims.resolve(0, c).id);
        if let Some(clip) = anims.find(stand) {
            let mut player = AnimationPlayer::default();
            match motion {
                BoothMotion::Frozen => {
                    player.play(clip.node).pause();
                }
                BoothMotion::Loop => {
                    player.play(clip.node).repeat();
                }
            }
            // A weapon hand plays its `HandsClosed` finger overlay over Stand, repeated because it
            // is a one-key clamp pose; armed once, since a bake's grip never changes.
            for (hand, want) in grip.into_iter().enumerate() {
                if let (true, Some(node)) = (want, anims.hand_close[hand]) {
                    let active = player.play(node);
                    active.repeat();
                    active.set_weight(crate::creature_anim::HAND_GRIP_WEIGHT);
                }
            }
            commands.entity(root).insert((
                player,
                AnimationGraphHandle(anims.graph.clone()),
                anims.clone(),
            ));
        }
    }
    booth_rig
}

/// The ids `PlayerModel:SetRotation` chooses between, from the reference's name table
/// (`0x6143b6`, `[ecx*4+0x8686a8]`): 0 `Stand`, 11 `ShuffleLeft`, 12 `ShuffleRight`.
const STAND: u16 = 0;
const SHUFFLE_LEFT: u16 = 11;
const SHUFFLE_RIGHT: u16 = 12;

/// The sequence a facing change arms (`0x505bb0`, the `fcomp` at `0x505bce`): current facing below
/// the angle arms ShuffleRight, above it ShuffleLeft; equal or NaN arms Stand, an active play,
/// which is why `Model:SetSequence` cannot stick on these panes. Compared on the Lua-facing
/// scalar `SetRotation` takes, so no sign conversion is involved.
pub(super) fn turn_shuffle(faced: f32, angle: f32) -> u16 {
    if faced < angle {
        SHUFFLE_RIGHT
    } else if faced > angle {
        SHUFFLE_LEFT
    } else {
        STAND
    }
}

/// How long one rotation keeps its shuffle stepping: the reference's `[+0x3ec] = clock() + 100`
/// (`0x42c010`, milliseconds), drained per paint by `0x505c50`. Every path through `0x505bb0` sets
/// it (`0x505c28`), so a held arrow pushes it out each frame. Taking `spun` before the expiry
/// check matches the `[+0x3e8]` latch: a paint after a `SetRotation` never expires.
const SHUFFLE_HOLD_SECS: f64 = 0.100;

/// The fraction of a cross-fade still to run, 1 to 0: the client's `(blendEnd − now) · blendRate`,
/// recomputed every frame. The per-frame weights and the half-blend refusal must agree on it.
fn fade_frac(fade: &super::Fade, now: f64) -> f32 {
    ((fade.until - now) / f64::from(fade.span)) as f32
}

/// Arms one `AnimationData` id as the reference's turn does,
/// `0x7121a0(bone -1, id, variation -1, offset 0, rate 1.0, blend 1, primary 1)`:
///
/// - variation -1: a frequency-weighted roll on every arm, expiry included (HumanMale authors
///   four Stands).
/// - offset 0: from the top of the clip.
/// - blend 1: a cross-fade over the incoming clip's `M2Sequence.blendTime` ([`super::Fade`]),
///   on HumanMale 0.25 s into a shuffle and 0.5 s back to Stand.
///
/// Returns false, scheduling nothing, when the display does not author the id (most pets for the
/// shuffles): the doll stays standing.
fn arm_turn(
    player: &mut AnimationPlayer,
    anims: &benilla_assets::ModelAnimations,
    catalog: &benilla_formats::AnimDataCatalog,
    rng: &mut benilla_assets::AnimRng,
    turn: &mut super::Turn,
    id: u16,
    now: f64,
) -> bool {
    // The pose this arm fades out of: the last arm's clip, or before the first turn the bake's own
    // Stand, the head variation.
    let outgoing = turn
        .playing
        .or_else(|| anims.find(anims.resolve(STAND, catalog).id).map(|c| c.node));
    let res = anims.resolve(id, catalog);
    if res.id != id {
        return false;
    }
    let Some(clip) = anims.pick_variation(res.id, rng.draw()) else {
        return false;
    };
    let (node, looping, blend) = (clip.node, clip.looping, clip.blend_time.max(0.0));
    // The half-blend refusal (`0x7125c9`/`0x7125d4`): a running blend with λ > 0.5 is not
    // re-seeded; the old secondary keeps its window and the primary is dropped. λ equal to 0.5
    // re-seeds. A second arrow nudge within 250 ms of a release meets it.
    let refused = turn
        .fade
        .is_some_and(|f| crate::creature_anim::select::blend_lambda(fade_frac(&f, now)) > 0.5);
    if refused {
        // The outgoing primary is dropped; the secondary keeps its older pose and λ.
        if let Some(out) = outgoing.filter(|o| *o != node) {
            player.stop(out);
        }
    } else if let Some(old) = turn.fade.take() {
        // One secondary slot: a fade in flight is displaced and stops drawing, unless it is the
        // node about to replay (a reversal back inside a running window).
        if old.node != node && Some(old.node) != outgoing {
            player.stop(old.node);
        }
    }
    let fade = if refused {
        turn.fade // untouched: old node, old window, old λ
    } else {
        outgoing
            .filter(|o| *o != node && blend > 0.0)
            .map(|node| super::Fade {
                node,
                until: now + f64::from(blend),
                span: blend,
            })
    };
    {
        // Set explicitly: `play` is idempotent on a live node, which would keep its old clock and
        // weight.
        let active = player.play(node);
        active.set_repeat(if looping {
            RepeatAnimation::Forever
        } else {
            RepeatAnimation::Never
        });
        active.set_speed(1.0);
        active.seek_to(0.0);
        // λ = 1 on a blended arm's first frame, so the pose does not move on the frame it lands.
        active.set_weight(if fade.is_some() { 0.0 } else { 1.0 });
    }
    if fade.is_none() {
        // Nothing was playing, or `blendTime = 0`: a cut, the client's `[blk+0xd0] = -1` path.
        if let Some(out) = outgoing.filter(|o| *o != node) {
            player.stop(out);
        }
    }
    turn.playing = Some(node);
    turn.fade = fade;
    true
}

/// Steps a pane doll's feet when it turns: `Model:SetRotation` arms a turn-in-place shuffle by
/// direction and then writes the facing. Both the shuffle and the return to Stand after the
/// 100 ms expiry are blended arms ([`arm_turn`]); the client's `rep movsd` copies the outgoing
/// track's clock, so the shuffle keeps stepping under the whole fade. Body panes only
/// ([`Booth::live`]); a booth root has no `AnimDriver`, so it fires no footstep sounds.
pub(super) fn drive_booth_turn(
    time: Res<Time<bevy::time::Real>>,
    mut booths: ResMut<super::Booths>,
    anim_data: Option<Res<crate::creature_anim::AnimData>>,
    mut rigs: Query<(&mut AnimationPlayer, &benilla_assets::ModelAnimations)>,
    // Variation -1 is a frequency-weighted roll off the CRT LCG (`0x71249a` calls `0x7400e5`),
    // the same generator the world's picks use.
    mut rng: ResMut<benilla_assets::AnimRng>,
) {
    let Some(anim_data) = anim_data.as_deref() else {
        return;
    };
    let now = time.elapsed_secs_f64();
    for booth in booths.0.values_mut() {
        if !booth.live {
            continue;
        }
        let Ok((mut player, anims)) = rigs.get_mut(booth.root) else {
            // No rig: a turn in flight names nodes of a player that is gone.
            booth.turn.rebaked();
            continue;
        };
        step_turn(
            &mut player,
            anims,
            &anim_data.0,
            &mut rng,
            &mut booth.turn,
            now,
        );
    }
}

/// One booth's turn for one frame, apart from the ECS so a test can step it at chosen times.
fn step_turn(
    player: &mut AnimationPlayer,
    anims: &benilla_assets::ModelAnimations,
    catalog: &benilla_formats::AnimDataCatalog,
    rng: &mut benilla_assets::AnimRng,
    turn: &mut super::Turn,
    now: f64,
) {
    // Arm: the reference compares the id against the root bone's (`0x712090`), so a held direction
    // arms once and loops while a reversal restarts from t = 0.
    if let Some(want) = turn.spun.take() {
        let armed = turn.shuffle.map_or(STAND, |(id, _)| id);
        if want == armed {
            // Same direction held, or Stand sent to a standing doll: only the deadline moves.
            if let Some((_, until)) = turn.shuffle.as_mut() {
                *until = now + SHUFFLE_HOLD_SECS;
            }
        } else if arm_turn(player, anims, catalog, rng, turn, want, now) {
            // Equal facings and NaN arm Stand (`0x505c23`), which schedules no expiry.
            turn.shuffle = (want != STAND).then_some((want, now + SHUFFLE_HOLD_SECS));
        }
    }
    // Expire: the arrow came up, so back to Stand, re-rolled and blended (`0x505c98`).
    if let Some((_, until)) = turn.shuffle {
        if now > until {
            arm_turn(player, anims, catalog, rng, turn, STAND, now);
            turn.shuffle = None;
        }
    }
    // Advance the cross-fade: λ, smoothstep of the window still to run (`0x714880`), weights the
    // outgoing pose and the incoming takes `1 − λ`, the client's
    // `out = primary + (secondary − primary)·λ`.
    let Some(fade) = turn.fade else {
        return;
    };
    let frac = fade_frac(&fade, now);
    if frac <= 0.0 {
        turn.fade = None;
        player.stop(fade.node);
        if let Some(cur) = turn.playing {
            player.play(cur).set_weight(1.0);
        }
    } else {
        let lambda = crate::creature_anim::select::blend_lambda(frac);
        player.play(fade.node).set_weight(lambda);
        if let Some(cur) = turn.playing {
            player.play(cur).set_weight(1.0 - lambda);
        }
    }
}

/// Faces each [`BoothBillboard`] to the booth camera sharing its render layer: the local rotation
/// cancels the joint's world rotation, which is read one frame stale.
pub(super) fn face_booth_billboards(
    cams: Query<(&GlobalTransform, &RenderLayers, &Camera), With<super::BoothCam>>,
    joints: Query<&GlobalTransform>,
    mut cards: Query<(&BoothBillboard, &ChildOf, &RenderLayers, &mut Transform)>,
) {
    for (card, child_of, layers, mut tf) in &mut cards {
        let Some((cam, _, _)) = cams
            .iter()
            // A parked camera's cards re-face on its first awake frame.
            .find(|(_, l, c)| c.is_active && l.intersects(layers))
        else {
            continue; // or the booth is torn down
        };
        let Ok(joint) = joints.get(child_of.parent()) else {
            continue;
        };
        let basis = benilla_world::billboard::billboard_basis(
            card.kind,
            Quat::IDENTITY,
            *cam.forward(),
            *cam.right(),
            *cam.up(),
        );
        tf.rotation = joint.rotation().inverse() * basis;
    }
}

/// Spends each [`BoothMatAlpha`] part's sampled material alpha, as the world's visibility pass
/// does: `A <= 0` hides it, since the reference culls before reading the blend mode and an opaque
/// draw ignores the tag alpha (the Voidwalker's shoulder props are keyed to 0 through Stand);
/// otherwise the alpha field is written alone, keeping the rig slot. Both write on change only.
/// `Visibility` is required, not optional: every marked part has a `Mesh3d`, which requires it.
pub(super) fn push_booth_mat_alpha(
    mut parts: Query<
        (
            &benilla_world::doodad_anim::MatAnim,
            &mut bevy::mesh::MeshTag,
            &mut Visibility,
        ),
        With<BoothMatAlpha>,
    >,
) {
    for (anim, mut tag, mut vis) in &mut parts {
        // `Inherited`, so a hidden bake root still hides its parts.
        let want = if anim.current > 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
        let bits = benilla_world::mesh_tag::with_alpha(tag.0, anim.current);
        if tag.0 != bits {
            tag.0 = bits;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_world::doodad_anim::MatAnim;

    /// A constant authored alpha, the shape the glue scenes use (UI_Tauren's vignette is 0.55).
    fn constant_alpha(v: f32) -> std::sync::Arc<benilla_formats::AlphaAnim> {
        std::sync::Arc::new(
            benilla_formats::AlphaAnim::new(vec![benilla_formats::AlphaSeq {
                color: None,
                weight: Some(benilla_formats::ScalarAnim {
                    period: 0.0,
                    step: true,
                    wrap: true, // period 0: a constant has no clock
                    gseq: false,
                    keys: vec![(0.0, v)],
                }),
            }])
            .expect("a dimming constant is worth carrying"),
        )
    }

    /// A writer that clobbered bits 19..=29 would unbind the skinned part's palette.
    #[test]
    fn the_booth_alpha_writer_preserves_the_rig_slot() {
        let mut app = App::new();
        app.add_systems(Update, push_booth_mat_alpha);
        let rig_slot = 7u16;
        let anim = MatAnim::driving_tag(constant_alpha(0.55), 0.0, None);
        let part = app
            .world_mut()
            .spawn((
                bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(rig_slot, 1.0)),
                anim,
                BoothMatAlpha,
                // A real booth part's `Mesh3d` requires `Visibility`, which the writer takes.
                Visibility::Inherited,
            ))
            .id();
        app.update();

        let tag = app
            .world()
            .entity(part)
            .get::<bevy::mesh::MeshTag>()
            .unwrap();
        assert_eq!(
            benilla_world::mesh_tag::rig_of(tag.0),
            rig_slot,
            "the palette slot must survive an alpha write"
        );
        let alpha = benilla_world::mesh_tag::alpha_of(tag.0);
        assert!(
            (alpha - 0.55).abs() <= 1.0 / 63.0,
            "authored 0.55 reached the tag (got {alpha})"
        );
    }

    /// The reference culls the batch before reading the blend mode (`0x707b3a` to `0x707b5c`).
    #[test]
    fn a_batch_the_artist_zeroed_in_this_sequence_is_culled_not_merely_dimmed() {
        let mut app = App::new();
        app.add_systems(Update, push_booth_mat_alpha);
        let spawn = |app: &mut App, v: f32| {
            app.world_mut()
                .spawn((
                    bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(0, 1.0)),
                    MatAnim::driving_tag(constant_alpha(v), 0.0, None),
                    BoothMatAlpha,
                    Visibility::Inherited,
                ))
                .id()
        };
        // The Voidwalker's Stand: the shoulder shackles keyed to 0, the wrist pair left at 1.
        let shoulder = spawn(&mut app, 0.0);
        let wrist = spawn(&mut app, 1.0);
        app.update();

        let vis = |app: &App, e: Entity| *app.world().entity(e).get::<Visibility>().unwrap();
        assert_eq!(
            vis(&app, shoulder),
            Visibility::Hidden,
            "A <= 0 is a disappearance, not a fade to nothing — an Opaque draw ignores the tag"
        );
        assert_eq!(
            vis(&app, wrist),
            Visibility::Inherited,
            "and a live batch stays drawable — Inherited, so a hidden bake root still wins"
        );

        // A batch keyed back on comes back.
        app.world_mut()
            .entity_mut(shoulder)
            .insert(MatAnim::driving_tag(constant_alpha(1.0), 0.0, None));
        app.update();
        assert_eq!(
            vis(&app, shoulder),
            Visibility::Inherited,
            "the cull tracks the sample; it does not latch"
        );
    }

    /// `LShoulder_Mail_PVPAlliance_C_01`'s billboard bone 1 pivot and sparkle emitter (raw WoW
    /// model space), under a posed joint whose rotation the frame must cancel: the emitter lands
    /// 0.24 yd behind the pivot along the booth camera's view axis and follows the camera.
    #[test]
    fn a_booth_billboard_frame_puts_an_item_emitter_behind_its_pivot() {
        use benilla_assets::coords::wow_to_bevy;
        const PIVOT: [f32; 3] = [-0.012, 0.162, -0.060];
        const EMITTER: [f32; 3] = [-0.252, 0.178, -0.046];
        // What `spawn_emitter`'s pivot rebase stores: the chain offset, raw WoW axes.
        let local = wow_to_bevy([
            EMITTER[0] - PIVOT[0],
            EMITTER[1] - PIVOT[1],
            EMITTER[2] - PIVOT[2],
        ]);

        let mut app = App::new();
        app.add_systems(Update, face_booth_billboards);
        let layer = RenderLayers::layer(9);
        // The booth camera, aimed off every world axis so nothing can pass by coincidence.
        let mut cam_tf = Transform::from_translation(Vec3::new(1.1, 1.6, 2.4))
            .looking_at(Vec3::new(0.0, 1.1, 0.0), Vec3::Y);
        let cam = app
            .world_mut()
            .spawn((
                crate::portrait::BoothCam("glue".to_string()),
                // The facer skips an inactive camera; the default is active.
                Camera::default(),
                GlobalTransform::from(cam_tf),
                layer.clone(),
            ))
            .id();
        // The item's host: the shoulder attach point under a joint holding a Stand-pose rotation.
        let host_tf = Transform {
            translation: Vec3::new(0.21, 1.42, 0.06),
            rotation: Quat::from_euler(EulerRot::YXZ, 0.7, -0.3, 0.2),
            scale: Vec3::ONE,
        };
        let host_gt = GlobalTransform::from(host_tf);
        let host = app.world_mut().spawn(host_gt).id();
        let frame = app
            .world_mut()
            .spawn((
                Transform::from_translation(wow_to_bevy(PIVOT)),
                layer.clone(),
                ChildOf(host),
                BoothBillboard::frame(BillboardKind::Spherical),
            ))
            .id();
        let pivot_world = host_gt.transform_point(wow_to_bevy(PIVOT));

        // The system writes the frame's local transform; this composes it as propagation would.
        let sparkle_offset = |app: &App| {
            let local_tf = *app.world().entity(frame).get::<Transform>().unwrap();
            let world = host_gt.mul_transform(local_tf).compute_transform();
            assert!(
                (world.translation - pivot_world).length() < 1e-5,
                "the frame sits AT the billboard pivot — only the rotation is replaced"
            );
            world.transform_point(local) - pivot_world
        };

        app.update();
        let fwd = *cam_tf.forward();
        let sparkle = sparkle_offset(&app);
        assert!(
            (sparkle.dot(fwd) - 0.240).abs() < 2e-3,
            "0.24 yd along the BOOTH camera's view axis, away from the eye: {sparkle:?}"
        );
        assert!(
            (sparkle - fwd * sparkle.dot(fwd)).length() < 0.025,
            "…and all but ~2 cm of the offset is in that one axis: {sparkle:?}"
        );

        // Move the booth camera, as the preview's yaw drag does relative to the model.
        cam_tf = Transform::from_translation(Vec3::new(-2.6, 1.0, 0.4))
            .looking_at(Vec3::new(0.0, 1.1, 0.0), Vec3::Y);
        app.world_mut()
            .entity_mut(cam)
            .insert(GlobalTransform::from(cam_tf));
        app.update();
        let turned = sparkle_offset(&app);
        let fwd2 = *cam_tf.forward();
        assert!(
            (turned.dot(fwd2) - 0.240).abs() < 2e-3
                && (turned - fwd2 * turned.dot(fwd2)).length() < 0.025,
            "the same 0.24 yd, now along the new view axis: {turned:?}"
        );
        assert!(
            (turned - sparkle).length() > 0.2,
            "…which means it MOVED — a rest-pose placement would not have"
        );
    }

    /// A static part draws at bind pose, so it keeps the ordinary test as the control.
    #[test]
    fn a_skinned_booth_part_is_never_frustum_culled() {
        let mut app = App::new();
        app.init_resource::<benilla_world::rig_palette::RigPalettes>();
        app.init_resource::<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>();

        // A two-bone rest skeleton, enough for `RigPose::new` and a real palette slot.
        let skeleton = benilla_assets::ModelSkeleton {
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
                    billboard: None,
                    parent_arm: None,
                },
            ],
            spine_bone: None,
            head_bone: None,
        };
        let ibp = Handle::default();
        // One skinned batch and one static one, through the same call.
        let parts = vec![
            BoothPart {
                skinned: Some(Handle::default()),
                static_mesh: Handle::default(),
                material: Handle::default(),
                alpha_anim: None,
                twins: BoothTwins::default(),
                mat_anim: false,
            },
            BoothPart {
                skinned: None,
                static_mesh: Handle::default(),
                material: Handle::default(),
                alpha_anim: None,
                twins: BoothTwins::default(),
                mat_anim: false,
            },
        ];

        let root = app.world_mut().spawn(Transform::IDENTITY).id();
        let mut palettes = app
            .world_mut()
            .remove_resource::<benilla_world::rig_palette::RigPalettes>()
            .expect("just inserted");
        let mut queue = bevy::ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, app.world());
            spawn_booth_model(
                &mut commands,
                &mut palettes,
                root,
                RenderLayers::layer(9),
                &parts,
                &[],
                Some((&skeleton, &ibp, None)),
                None,
                BoothMotion::Frozen,
                [false, false],
                &[],
                BoothInstance::default(),
            )
            .finish(&mut commands);
        }
        queue.apply(app.world_mut());
        app.world_mut().insert_resource(palettes);

        let children: Vec<Entity> = app
            .world()
            .entity(root)
            .get::<Children>()
            .expect("the bake spawned its parts")
            .iter()
            .collect();
        let rigged: Vec<Entity> = children
            .iter()
            .copied()
            .filter(|e| {
                app.world()
                    .entity(*e)
                    .contains::<benilla_world::rig_palette::RigPart>()
            })
            .collect();
        assert_eq!(rigged.len(), 1, "one of the two batches skins");
        assert!(
            app.world()
                .entity(rigged[0])
                .contains::<bevy::camera::visibility::NoFrustumCulling>(),
            "a palette-skinned booth part must not be tested against its bind-pose bound"
        );
        let statics: Vec<Entity> = children
            .iter()
            .copied()
            .filter(|e| !rigged.contains(e))
            .collect();
        assert!(
            statics.iter().all(|e| !app
                .world()
                .entity(*e)
                .contains::<bevy::camera::visibility::NoFrustumCulling>()),
            "…and the static twin keeps the ordinary test — its own box IS where it draws"
        );
    }

    /// World parts carry `MatAnim` too; their alpha belongs to the world's visibility pass.
    #[test]
    fn the_booth_alpha_writer_ignores_unmarked_parts() {
        let mut app = App::new();
        app.add_systems(Update, push_booth_mat_alpha);
        let part = app
            .world_mut()
            .spawn((
                bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(0, 1.0)),
                MatAnim::driving_tag(constant_alpha(0.1), 0.0, None),
            ))
            .id();
        app.update();
        assert_eq!(
            benilla_world::mesh_tag::alpha_of(
                app.world()
                    .entity(part)
                    .get::<bevy::mesh::MeshTag>()
                    .unwrap()
                    .0
            ),
            1.0,
            "no BoothMatAlpha marker ⇒ untouched"
        );
    }

    /// The stock Lua turns opposite ways in its two callers: a held left arrow adds
    /// (`Model_OnUpdate`, `UIParent.lua:1449`), a left click subtracts (`Model_RotateLeft`,
    /// `UIParent.lua:1429`).
    #[test]
    fn a_turn_arms_the_shuffle_for_the_way_it_turned() {
        assert_eq!(turn_shuffle(0.9, 1.0), SHUFFLE_RIGHT, "facing rose");
        assert_eq!(turn_shuffle(1.0, 0.9), SHUFFLE_LEFT, "facing fell");
        assert_eq!(turn_shuffle(0.61, 0.61), STAND, "a re-pose arms Stand");
        assert_eq!(
            turn_shuffle(0.61, f32::NAN),
            STAND,
            "unordered falls to Stand"
        );
        let facing = 0.61;
        // Held: left adds, so a held left arrow steps ShuffleRight.
        assert_eq!(
            turn_shuffle(facing, facing + 0.05),
            SHUFFLE_RIGHT,
            "held left"
        );
        assert_eq!(
            turn_shuffle(facing, facing - 0.05),
            SHUFFLE_LEFT,
            "held right"
        );
        // Clicked: left subtracts, so the same arrow lands on the other shuffle.
        assert_eq!(
            turn_shuffle(facing, facing - 0.03),
            SHUFFLE_LEFT,
            "clicked left"
        );
        assert_eq!(
            turn_shuffle(facing, facing + 0.03),
            SHUFFLE_RIGHT,
            "clicked right"
        );
    }

    // ── The turn's cross-fades ────────────────────────────────────────────

    /// The three ids a turn arms, with HumanMale's authored blend times (shuffles 0.25 s, Stand
    /// 0.5 s). Two Stands, weighted so a roll never lands on the head, which `find` would return.
    fn turning_model() -> benilla_assets::ModelAnimations {
        let clip = |anim_id, node, blend_time, frequency| benilla_assets::AnimClip {
            anim_id,
            seq_index: 0,
            node: bevy::animation::graph::AnimationNodeIndex::new(node),
            looping: true,
            duration: 0.5,
            move_speed: 0.0,
            blend_time,
            bounds_center: Vec3::ZERO,
            bounds_radius: 0.0,
            bounds_min: Vec3::ZERO,
            bounds_max: Vec3::ZERO,
            events: Vec::new().into(),
            arm_nodes: None,
            upper_node: None,
            frequency,
            replay: (0, 0),
            poses_bones: true,
        };
        benilla_assets::ModelAnimations {
            graph: Handle::default(),
            clips: vec![
                clip(STAND, 1, 0.5, 1),         // the head, all but unreachable by the roll
                clip(STAND, 2, 0.5, 32766),     // where a roll lands
                clip(SHUFFLE_LEFT, 3, 0.25, 0), // the only variation
                clip(SHUFFLE_RIGHT, 4, 0.25, 0),
            ],
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }

    fn node(n: usize) -> bevy::animation::graph::AnimationNodeIndex {
        bevy::animation::graph::AnimationNodeIndex::new(n)
    }

    /// The bake's own arm, Stand's head variation looping, which the first turn fades out of.
    fn baked() -> AnimationPlayer {
        let mut player = AnimationPlayer::default();
        player.play(node(1)).repeat();
        player
    }

    fn weight(player: &AnimationPlayer, n: usize) -> Option<f32> {
        player.animation(node(n)).map(|a| a.weight())
    }

    /// Every arm `0x505bb0`/`0x505c50` makes carries `blendFlag = 1`, each over the incoming clip's
    /// `blendTime`; the weights sum to 1 throughout.
    #[test]
    fn a_turn_blends_both_ways_over_the_incoming_clips_own_time() {
        let (anims, catalog) = (
            turning_model(),
            benilla_formats::AnimDataCatalog::from_rows([]),
        );
        let (mut player, mut rng) = (baked(), benilla_assets::AnimRng::default());
        let mut turn = super::super::Turn {
            // Arrow down: the shuffle is armed but contributes nothing yet.
            spun: Some(SHUFFLE_LEFT),
            ..Default::default()
        };
        step_turn(&mut player, &anims, &catalog, &mut rng, &mut turn, 1.0);
        assert_eq!(turn.playing, Some(node(3)), "the shuffle is the primary");
        assert_eq!(weight(&player, 3), Some(0.0), "and it starts at nothing");
        assert_eq!(
            weight(&player, 1),
            Some(1.0),
            "the bake's Stand still holds"
        );

        // Mid-blend, on the shuffle's 0.25 s.
        turn.spun = Some(SHUFFLE_LEFT);
        step_turn(&mut player, &anims, &catalog, &mut rng, &mut turn, 1.2);
        let (out, inc) = (weight(&player, 1).unwrap(), weight(&player, 3).unwrap());
        assert!((out + inc - 1.0).abs() < 1e-5, "{out} + {inc}");
        assert!(inc > 0.8, "four fifths in, the shuffle dominates: {inc}");

        // Past 0.25 s the fade is done and the pose it faded out of is gone from the player.
        turn.spun = Some(SHUFFLE_LEFT);
        step_turn(&mut player, &anims, &catalog, &mut rng, &mut turn, 1.3);
        assert_eq!(weight(&player, 3), Some(1.0));
        assert_eq!(weight(&player, 1), None, "the outgoing Stand is stopped");
        assert!(turn.fade.is_none());

        // Arrow up: 100 ms later the expiry arms Stand, and this frame must not move the doll.
        step_turn(&mut player, &anims, &catalog, &mut rng, &mut turn, 1.41);
        let stand = turn.playing.expect("Stand armed");
        assert_ne!(stand, node(1), "a fresh frequency roll, not `find`'s head");
        assert_eq!(stand, node(2));
        assert_eq!(
            weight(&player, 2),
            Some(0.0),
            "the release frame does not jump"
        );
        assert_eq!(
            weight(&player, 3),
            Some(1.0),
            "the shuffle still holds the pose"
        );
        assert!(turn.shuffle.is_none());

        // It runs for Stand's 0.5 s, so it is still blending 0.3 s in.
        step_turn(&mut player, &anims, &catalog, &mut rng, &mut turn, 1.71);
        let (out, inc) = (weight(&player, 3).unwrap(), weight(&player, 2).unwrap());
        assert!((out + inc - 1.0).abs() < 1e-5, "{out} + {inc}");
        assert!(
            out > 0.0 && inc > 0.0,
            "both still contribute: {out} / {inc}"
        );

        step_turn(&mut player, &anims, &catalog, &mut rng, &mut turn, 1.92);
        assert_eq!(weight(&player, 2), Some(1.0), "settled on Stand");
        assert_eq!(
            weight(&player, 3),
            None,
            "the shuffle is stopped, not muted"
        );
    }

    /// Inside a blend's first half `0x7125d4` refuses the arm; at or past halfway the arm takes the
    /// one secondary slot.
    #[test]
    fn a_reversal_is_refused_inside_the_half_blend_and_displaces_it_after() {
        let (anims, catalog) = (
            turning_model(),
            benilla_formats::AnimDataCatalog::from_rows([]),
        );

        // Reverse at 0.1 s of a 0.25 s blend: 60% still to run, λ = 0.648, refused.
        let (mut player, mut rng) = (baked(), benilla_assets::AnimRng::default());
        let mut turn = super::super::Turn {
            spun: Some(SHUFFLE_LEFT),
            ..Default::default()
        };
        step_turn(&mut player, &anims, &catalog, &mut rng, &mut turn, 1.0);
        turn.spun = Some(SHUFFLE_RIGHT);
        step_turn(&mut player, &anims, &catalog, &mut rng, &mut turn, 1.1);

        assert_eq!(
            turn.playing,
            Some(node(4)),
            "the opposite shuffle took over"
        );
        assert_eq!(
            turn.fade.map(|f| (f.node, f.until)),
            Some((node(1), 1.25)),
            "the ORIGINAL fade is untouched — same pose, same window"
        );
        assert_eq!(
            weight(&player, 3),
            None,
            "the first shuffle is dropped, not faded"
        );
        assert_eq!(turn.shuffle.map(|(id, _)| id), Some(SHUFFLE_RIGHT));

        // Reverse at 0.2 s instead: 20% still to run, λ = 0.104, the slot is taken.
        let (mut player, mut rng) = (baked(), benilla_assets::AnimRng::default());
        let mut turn = super::super::Turn {
            spun: Some(SHUFFLE_LEFT),
            ..Default::default()
        };
        step_turn(&mut player, &anims, &catalog, &mut rng, &mut turn, 1.0);
        turn.spun = Some(SHUFFLE_RIGHT);
        step_turn(&mut player, &anims, &catalog, &mut rng, &mut turn, 1.2);

        assert_eq!(
            turn.fade.map(|f| f.node),
            Some(node(3)),
            "…fading out of the first"
        );
        assert_eq!(weight(&player, 1), None, "the displaced Stand is gone");
    }

    /// Stand blends in over 0.5 s, so a nudge within 250 ms of a release meets a blend more than
    /// half to run.
    #[test]
    fn a_second_nudge_inside_stands_own_blend_is_refused() {
        let (anims, catalog) = (
            turning_model(),
            benilla_formats::AnimDataCatalog::from_rows([]),
        );
        let (mut player, mut rng) = (baked(), benilla_assets::AnimRng::default());
        let mut turn = super::super::Turn {
            spun: Some(SHUFFLE_LEFT),
            ..Default::default()
        };
        step_turn(&mut player, &anims, &catalog, &mut rng, &mut turn, 1.0);
        turn.spun = Some(SHUFFLE_LEFT);
        step_turn(&mut player, &anims, &catalog, &mut rng, &mut turn, 1.1);
        // Released: the expiry arms Stand, fading out of the shuffle over Stand's own 0.5 s.
        step_turn(&mut player, &anims, &catalog, &mut rng, &mut turn, 1.3);
        let settling = turn.playing.expect("Stand armed");
        assert_eq!(turn.fade.map(|f| (f.node, f.until)), Some((node(3), 1.8)));

        // Nudged again 100 ms later: 80% of Stand's blend still to run, λ = 0.896, refused.
        turn.spun = Some(SHUFFLE_RIGHT);
        step_turn(&mut player, &anims, &catalog, &mut rng, &mut turn, 1.4);

        assert_eq!(
            turn.playing,
            Some(node(4)),
            "the new shuffle is the primary"
        );
        assert_eq!(
            turn.fade.map(|f| (f.node, f.until)),
            Some((node(3), 1.8)),
            "still fading the shuffle it was already fading, on the same window"
        );
        assert_eq!(
            player.animation(settling).map(|a| a.weight()),
            None,
            "the half-faded Stand is dropped, not layered on"
        );
    }

    #[test]
    fn a_display_with_no_shuffle_never_leaves_stand() {
        let mut anims = turning_model();
        anims.clips.retain(|c| c.anim_id == STAND);
        // The model's own baked resolution: both shuffles fall back to Stand.
        anims.playable_animation_lookup = vec![
            benilla_formats::PlayableAnim {
                resolved_id: STAND,
                dir_flags: 0,
            };
            usize::from(SHUFFLE_RIGHT) + 1
        ];
        let catalog = benilla_formats::AnimDataCatalog::from_rows([]);
        let (mut player, mut rng) = (baked(), benilla_assets::AnimRng::default());
        let mut turn = super::super::Turn::default();
        for t in [1.0, 1.1, 1.2, 2.0] {
            turn.spun = Some(SHUFFLE_LEFT);
            step_turn(&mut player, &anims, &catalog, &mut rng, &mut turn, t);
        }
        assert!(turn.shuffle.is_none(), "no expiry was ever scheduled");
        assert!(turn.fade.is_none(), "and nothing was cross-faded");
        assert_eq!(weight(&player, 1), Some(1.0), "the bake's Stand, untouched");
    }
}

#[cfg(test)]
mod instance_tests {
    use super::*;

    /// Three unequal handles.
    fn handles() -> (
        Handle<WowModelMaterial>,
        Handle<WowModelMaterial>,
        Handle<WowModelMaterial>,
    ) {
        let assets = Assets::<WowModelMaterial>::default();
        let one = || assets.reserve_handle();
        (one(), one(), one())
    }

    #[test]
    fn an_opaque_bake_draws_steady_and_arms_no_depth_prime() {
        let (steady, blend, zfill) = handles();
        let twins = BoothTwins {
            blend: Some(blend),
            zfill: Some(zfill),
        };
        let opaque = BoothInstance::default();
        assert!(!opaque.feathering());
        assert_eq!(opaque.material(&steady, &twins), steady);
        assert!(opaque.fade_materials(&steady, &twins).is_none());
    }

    /// The zfill twin keeps an arm over the torso from compounding 0.5 over 0.5 (`M2UseZFill`).
    #[test]
    fn a_ghosted_bake_takes_the_blend_twin_and_carries_the_zfill() {
        let (steady, blend, zfill) = handles();
        let twins = BoothTwins {
            blend: Some(blend.clone()),
            zfill: Some(zfill.clone()),
        };
        let ghost = BoothInstance { alpha: 0.5 };
        assert!(ghost.feathering());
        assert_eq!(ghost.material(&steady, &twins), blend);
        let fm = ghost
            .fade_materials(&steady, &twins)
            .expect("a feathering batch with a twin carries the record");
        assert_eq!(fm.cutout, steady, "the steady material it settles back to");
        assert_eq!(fm.blend, blend);
        assert_eq!(fm.zfill, Some(zfill));
        assert!(
            fm.bake_blend.is_none(),
            "a booth is never interior-classified"
        );
    }

    /// An attached model's emitters compose onto the instance alpha (`0x714260`, into
    /// `emitter+0x1a8`); the fold is tested in `benilla_world`, this pins the gate.
    #[test]
    fn a_feathering_bake_hands_its_effect_host_an_alpha_to_compose() {
        assert!(BoothInstance { alpha: 0.5 }.feathering());
        assert!(
            !BoothInstance::default().feathering(),
            "and an opaque bake hands it none, so every existing booth is untouched"
        );
    }

    /// A Mod/Mod2x blend reads no alpha, so no swap can feather it.
    #[test]
    fn a_twinless_batch_stays_steady_even_while_the_instance_feathers() {
        let (steady, _, _) = handles();
        let none = BoothTwins::default();
        let ghost = BoothInstance { alpha: 0.5 };
        assert_eq!(ghost.material(&steady, &none), steady);
        assert!(ghost.fade_materials(&steady, &none).is_none());
    }
}
