//! The booth bake spawn: a mirrored look becomes the posed throwaway instance the camera shoots
//! (the reference's instance `0x707400`, pose `0x7121a0`). The skeleton is one
//! [`RigPose`](benilla_world::rig_anim::RigPose) buffer on the booth root; riders, cards and
//! effect hosts seat on demand-spawned anchors ([`BoothRig::anchor`]).

use benilla_formats::BillboardKind;
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
    /// True only on the glue stage (its scene, character and pet), which animates texture
    /// transforms per frame (`0x715f25` to `0x7163bc` over `md+0x74`) on the stage's own clock;
    /// portraits and panes are stills.
    pub(super) mat_anim: bool,
}

/// Marks a booth child for the draw gate when its material is on the UV and tint lane; an
/// unmarked registered batch stays frozen at its seed.
fn mark_mat_anim(child: &mut bevy::ecs::system::EntityCommands<'_>, mat_anim: bool) {
    if mat_anim {
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
    /// As [`BoothPart::mat_anim`]: its material has UV or tint rows of its own.
    pub(super) mat_anim: bool,
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
    /// As [`BoothPart::mat_anim`].
    pub(super) mat_anim: bool,
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
    pub(super) rig: Option<super::EffectRig>,
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
        // The model's idle poses the bones its emitters ride, as the world's item does.
        let mut pose = fx.rig.as_ref().map(|r| {
            let mut player = AnimationPlayer::default();
            if let Some(idle) = r.animations.idle_clip() {
                let active = player.play(idle.node);
                if idle.looping {
                    active.repeat();
                }
            }
            commands.entity(host).insert((
                player,
                AnimationGraphHandle(r.animations.graph.clone()),
                r.animations.clone(),
            ));
            benilla_world::rig_anim::RigPose::new(host, &r.skeleton).without_camera_billboards()
        });
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
                None => pose
                    .as_mut()
                    .and_then(|p| p.anchor_for(commands, host, em.def.bone))
                    .map_or((host, [0.0; 3]), |anchor| (anchor, em.bone_pivot)),
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
        if let Some(pose) = pose {
            commands.entity(host).insert((pose, super::StageRig));
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

/// Marks one of a bake's own batches, a direct child of the booth root, so a re-dress
/// ([`redress_booth_batches`]) tells them from the rig's anchors.
#[derive(Component)]
pub(super) struct BoothBatch;

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
        // No palette slot: every part draws its static twin at bind pose.
        for p in parts {
            spawn_batch(commands, root, &layer, p, 0, instance, None);
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
    // A live booth's batch alphas follow the sequence its root plays, on its clock; a still's
    // stay pinned.
    let host = (motion == BoothMotion::Loop && anims.is_some()).then_some(root);
    for p in parts {
        spawn_batch(commands, root, &layer, p, rig_slot, instance, host);
    }
    let mut booth_rig = BoothRig {
        root,
        rig: Some(pose),
        slot: rig_slot,
    };
    hang_attachments(
        commands,
        &mut booth_rig,
        &layer,
        riders,
        billboards,
        instance,
    );
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
            set_grip(&mut player, anims.hand_close, grip);
            commands.entity(root).insert((
                player,
                AnimationGraphHandle(anims.graph.clone()),
                anims.clone(),
            ));
        }
    }
    booth_rig
}

/// The mesh a batch draws against palette slot `rig_slot`: its skinned twin when it skins there,
/// else its static one (slot 0 is no rig).
fn batch_mesh(p: &BoothPart, rig_slot: u16) -> &Handle<Mesh> {
    match &p.skinned {
        Some(skinned) if rig_slot != 0 => skinned,
        _ => &p.static_mesh,
    }
}

/// Spawns one batch under `root`, skinned against `rig_slot` when it has a skinned twin.
fn spawn_batch(
    commands: &mut Commands,
    root: Entity,
    layer: &RenderLayers,
    p: &BoothPart,
    rig_slot: u16,
    instance: BoothInstance,
    host: Option<Entity>,
) -> Entity {
    let use_rig = p.skinned.is_some() && rig_slot != 0;
    let mut child = commands.spawn((
        Mesh3d(batch_mesh(p, rig_slot).clone()),
        Transform::IDENTITY,
        layer.clone(),
        ChildOf(root),
        BoothBatch,
    ));
    dress_batch(&mut child, p, rig_slot, instance, host);
    if use_rig {
        child.insert(benilla_world::rig_palette::RigPart(root));
        // A skinned booth part is never frustum-culled: Bevy's bound is the bind-pose box, the
        // camera frames the Stand pose, and on a model whose rest pose is far from Stand
        // (`Creature\CarrionBird`, bind z to 1.19, Stand z 0.54 to 6.23) every batch culls.
        child.insert(bevy::camera::visibility::NoFrustumCulling);
    }
    child.id()
}

/// A batch's dressing: its material, its tag (the palette slot beside the instance alpha, or the
/// material alpha that drives it) and its depth-prime record. `host` is the root whose player the
/// material alpha follows, its sequence and its scene's clock; `None` pins it.
fn dress_batch(
    child: &mut bevy::ecs::system::EntityCommands<'_>,
    p: &BoothPart,
    rig_slot: u16,
    instance: BoothInstance,
    host: Option<Entity>,
) {
    child.insert(MeshMaterial3d(instance.material(&p.material, &p.twins)));
    mark_mat_anim(child, p.mat_anim);
    // The material alpha drives the tag's alpha field itself (nothing else writes a booth part's
    // alpha) beside the rig slot. `alpha_bits` floors a true zero just above 0, so the shader's
    // all-zero "untagged, opaque" sentinel cannot fire on an invisible batch.
    let use_rig = p.skinned.is_some() && rig_slot != 0;
    let tag_slot = if use_rig { rig_slot } else { 0 };
    if let Some(anim) = &p.alpha_anim {
        // The batch's material alpha times the instance alpha, the reference's
        // `model+0x180 × the batch factor` (`0x707680`).
        let mat_anim = benilla_world::doodad_anim::MatAnim::driving_tag(anim.clone(), 0.0, None)
            .following_host(host);
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
}

/// Seats the riders on their bones' anchors and the camera-facing cards under theirs; a bone the
/// rig lacks drops its rider or card.
fn hang_attachments(
    commands: &mut Commands,
    booth_rig: &mut BoothRig,
    layer: &RenderLayers,
    riders: &[BoothRider],
    billboards: &[BoothBillboardSpec],
    instance: BoothInstance,
) {
    let rig_slot = booth_rig.slot;
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
        mark_mat_anim(&mut child, r.mat_anim);
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
    // Camera-facing batches seat under their bone's anchor for `face_booth_billboards` to face.
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
        mark_mat_anim(&mut card, bb.mat_anim);
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
}

/// Re-dresses the instance standing on `root` without replacing it, as the reference's setters
/// act on one `CharacterModel`: the skin setter `0x478790` rebinds its textures and raises the
/// dirty byte `[cm+8]`, and the class re-dress `0x470800` re-equips the same model. The root keeps
/// its player, global sequences and palette slot `rig_slot`, so its clock runs on. A batch whose
/// mesh is still worn keeps its entity and takes its new dressing; the others spawn or go, as a
/// geoset toggles. `standing` is the root's [`BoothBatch`] children by the mesh each draws.
///
/// Opaque instances only: a feathering batch's depth-prime twin belongs to the zfill lane, which
/// does not reap it when the record goes.
pub(super) fn redress_booth_batches(
    commands: &mut Commands,
    root: Entity,
    layer: &RenderLayers,
    parts: &[BoothPart],
    standing: &[(Entity, AssetId<Mesh>)],
    rig_slot: u16,
    instance: BoothInstance,
) {
    debug_assert!(
        !instance.feathering(),
        "a re-dress keeps opaque batches only"
    );
    let mut free: Vec<Option<(Entity, AssetId<Mesh>)>> =
        standing.iter().copied().map(Some).collect();
    for p in parts {
        let want = batch_mesh(p, rig_slot).id();
        let kept = free
            .iter_mut()
            .find(|s| s.is_some_and(|(_, mesh)| mesh == want))
            .and_then(Option::take);
        match kept {
            Some((batch, _)) => {
                let mut child = commands.entity(batch);
                // Strip the old dressing; `Visibility` too, which the material-alpha writer owns.
                child
                    .remove::<(
                        bevy::mesh::MeshTag,
                        benilla_world::doodad_anim::MatAnim,
                        BoothMatAlpha,
                        benilla_world::doodad_anim::AnimMatPart,
                    )>()
                    .insert(Visibility::Inherited);
                dress_batch(&mut child, p, rig_slot, instance, Some(root));
            }
            None => {
                spawn_batch(commands, root, layer, p, rig_slot, instance, Some(root));
            }
        }
    }
    for (batch, _) in free.into_iter().flatten() {
        commands.entity(batch).despawn();
    }
}

impl BoothRig {
    /// A fresh bind-pose buffer for the instance standing on `root`, on its palette slot `slot`:
    /// a re-dress re-hangs its attachments on new anchors ([`Self::hang`]), and the pose
    /// evaluator re-poses it from the root's player before it draws.
    pub(super) fn rehang(
        root: Entity,
        skeleton: &benilla_assets::ModelSkeleton,
        slot: u16,
    ) -> Self {
        Self {
            root,
            rig: Some(
                benilla_world::rig_anim::RigPose::new(root, skeleton).without_camera_billboards(),
            ),
            slot,
        }
    }

    /// Seats `riders` and `billboards` on this rig ([`hang_attachments`]).
    pub(super) fn hang(
        &mut self,
        commands: &mut Commands,
        layer: &RenderLayers,
        riders: &[BoothRider],
        billboards: &[BoothBillboardSpec],
        instance: BoothInstance,
    ) {
        hang_attachments(commands, self, layer, riders, billboards, instance);
    }
}

/// Closes each hand whose attach point holds a weapon (`CloseHand` `0x479660`) and opens the
/// other, as an unequip's detach does (`0x47a380`): the `HandsClosed` finger overlay over Stand,
/// repeated because it is a one-key clamp pose.
pub(super) fn set_grip(
    player: &mut AnimationPlayer,
    hand_close: [Option<bevy::animation::graph::AnimationNodeIndex>; 2],
    grip: [bool; 2],
) {
    for (hand, want) in grip.into_iter().enumerate() {
        let Some(node) = hand_close[hand] else {
            continue;
        };
        if !want {
            player.stop(node);
        } else if player.animation(node).is_none() {
            let active = player.play(node);
            active.repeat();
            active.set_weight(crate::creature_anim::HAND_GRIP_WEIGHT);
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

    /// A live booth's batch alpha follows its root's player, its sequence and its scene's clock,
    /// so a glue scene's dimming loops stop with the scene; a still's stays pinned.
    #[test]
    fn a_live_booths_batch_alpha_follows_its_root() {
        let skeleton = benilla_assets::ModelSkeleton {
            joints: vec![benilla_assets::ModelJoint {
                parent: -1,
                local_translation: Vec3::ZERO,
                billboard: None,
                parent_arm: None,
            }],
            spine_bone: None,
            head_bone: None,
        };
        let anims = benilla_assets::ModelAnimations {
            graph: Handle::default(),
            clips: vec![benilla_assets::AnimClip {
                anim_id: 0,
                seq_index: 0,
                node: bevy::animation::graph::AnimationNodeIndex::new(1),
                looping: true,
                duration: 0.5,
                move_speed: 0.0,
                blend_time: 0.5,
                bounds_center: Vec3::ZERO,
                bounds_radius: 0.0,
                bounds_min: Vec3::ZERO,
                bounds_max: Vec3::ZERO,
                events: Vec::new().into(),
                arm_nodes: None,
                upper_node: None,
                frequency: 32767,
                replay: (0, 0),
                poses_bones: true,
            }],
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        };
        for (motion, follows) in [(BoothMotion::Loop, true), (BoothMotion::Frozen, false)] {
            let mut app = App::new();
            app.init_resource::<benilla_world::rig_palette::RigPalettes>();
            let parts = vec![BoothPart {
                skinned: None,
                static_mesh: Handle::default(),
                material: Handle::default(),
                alpha_anim: Some(constant_alpha(0.55)),
                twins: BoothTwins::default(),
                mat_anim: false,
            }];
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
                    Some((&skeleton, &Handle::default(), Some(&anims))),
                    None,
                    motion,
                    [false, false],
                    &[],
                    BoothInstance::default(),
                )
                .finish(&mut commands);
            }
            queue.apply(app.world_mut());
            let mut q = app.world_mut().query::<&MatAnim>();
            let hosts: Vec<_> = q.iter(app.world()).map(MatAnim::host).collect();
            assert_eq!(
                hosts,
                vec![follows.then_some(root)],
                "{} booth",
                if follows { "a live" } else { "a still" }
            );
        }
    }

    /// An effect model with a rig poses its bones by its idle, so its emitter rides its bone's
    /// anchor on the effect host, as the world's item does.
    #[test]
    fn a_rigged_effect_model_poses_the_bone_its_emitter_rides() {
        let mut app = App::new();
        app.init_resource::<benilla_world::rig_palette::RigPalettes>();
        app.init_resource::<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>();
        let joint = |parent, local_translation| benilla_assets::ModelJoint {
            parent,
            local_translation,
            billboard: None,
            parent_arm: None,
        };
        let skeleton = benilla_assets::ModelSkeleton {
            joints: vec![joint(-1, Vec3::ZERO), joint(0, Vec3::Y)],
            spine_bone: None,
            head_bone: None,
        };
        let animations = benilla_assets::ModelAnimations {
            graph: Handle::default(),
            clips: Vec::new(),
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        };
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
        let effects = [BoothEffects {
            bone: 1,
            offset: Vec3::ZERO,
            emitters: vec![em],
            rig: Some(super::super::EffectRig {
                skeleton: skeleton.clone(),
                animations,
            }),
        }];
        let ibp = Handle::default();

        let root = app.world_mut().spawn(Transform::IDENTITY).id();
        let mut palettes = app
            .world_mut()
            .remove_resource::<benilla_world::rig_palette::RigPalettes>()
            .expect("just inserted");
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let spawned = {
            let mut commands = Commands::new(&mut queue, app.world());
            let mut rig = spawn_booth_model(
                &mut commands,
                &mut palettes,
                root,
                RenderLayers::layer(9),
                &[],
                &[],
                Some((&skeleton, &ibp, None)),
                None,
                BoothMotion::Frozen,
                [false, false],
                &[],
                BoothInstance::default(),
            );
            let (spawned, _) = spawn_booth_effects(
                &mut commands,
                &mut rig,
                &RenderLayers::layer(9),
                None,
                &effects,
                BoothInstance::default(),
            );
            rig.finish(&mut commands);
            spawned
        };
        queue.apply(app.world_mut());
        assert_eq!(spawned, 1);

        let host = app
            .world_mut()
            .query_filtered::<Entity, (
                With<benilla_world::rig_anim::RigPose>,
                With<AnimationPlayer>,
                With<super::super::StageRig>,
            )>()
            .iter(app.world())
            .find(|&e| e != root)
            .expect("the effect host is posed by its idle, outside the world parker");
        let anchors: Vec<u16> = app
            .world_mut()
            .query::<&benilla_world::rig_anim::RigAnchor>()
            .iter(app.world())
            .filter(|a| a.rig == host)
            .map(|a| a.bone)
            .collect();
        assert_eq!(anchors, vec![1], "the emitter rides its own bone's anchor");
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
