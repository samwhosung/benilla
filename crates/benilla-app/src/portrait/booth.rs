//! The booth **bake spawn** — how a mirrored look becomes the posed throwaway instance the
//! camera shoots (the ref mechanism: instance `0x707400`, pose `0x7121a0`). [`spawn_booth_model`]
//! is the whole surface; the sync systems in [`super`] build [`BoothPart`]/[`BoothRider`] lists
//! from the unit's mirrored children and hand them here.
//!
//! Since decision 1443 the bake rides the **collapsed rig lane** (0724/1365): the skeleton is a
//! [`RigPose`](benilla_world::rig_anim::RigPose) buffer on the booth root — no joint entities,
//! no per-bone `AnimatedBy` seats in bevy's `animate_targets` sweep (the booth doll was the last
//! consumer of that lane; ~0.35 ms/frame of the open char window). Consumers
//! (riders, cards, effect hosts) seat on demand-spawned anchors ([`BoothRig::anchor`]), and the
//! park is one [`AnimParked`](benilla_world::rig_anim::AnimParked) marker on the root.

use benilla_formats::BillboardKind;
use bevy::animation::RepeatAnimation;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;
use benilla_world::model_fade::FadeMaterials;

/// One mesh headed into a booth bake: the mirrored part's twins + its studio-lit material.
pub(super) struct BoothPart {
    pub(super) skinned: Option<Handle<Mesh>>,
    pub(super) static_mesh: Handle<Mesh>,
    pub(super) material: Handle<WowModelMaterial>,
    /// The batch's **animated material alpha** (`ModelSubmesh::alpha_anim`) — a time-varying
    /// colour-alpha/weight loop or, far more often here, a plain authored dimming *constant*. Every
    /// world lane has sampled this into the render-alpha `MeshTag` since decision 0130 phase 2; the
    /// booths never did, so a booth batch drew at alpha 1.0 whatever the artist authored (decision
    /// 0807 — B121's black corners were UI_Tauren's 0.55 `LENSALPHA` vignette at full strength).
    /// `None` for the overwhelming majority of batches, which are opaque.
    pub(super) alpha_anim: Option<std::sync::Arc<benilla_formats::AlphaAnim>>,
    /// The batch's translucency twins, for a bake whose **instance alpha** is below 1
    /// ([`BoothInstance`]) — the blend twin it draws through and the depth-prime twin that keeps
    /// it one layer. Both already exist on every M2 batch (`EntityPart::fade_blend`/`zfill`,
    /// the char composite's quint); the booth relights them beside the steady one and hands them
    /// here. `None` for a batch that cannot feather at all — the reference's own twin gate.
    pub(super) twins: BoothTwins,
    /// Whether this batch's material was put on the **UV / tint lane** by the builder that made it
    /// — i.e. whether [`benilla_world::doodad_anim::AnimMatPart`] belongs on the spawned child.
    ///
    /// **A statement about what was registered, never a re-test of the asset** (decision 2295's
    /// lesson, 2038's bug class before it): the marker and the registration have to be one fact,
    /// or the draw gate marks batches nothing samples — or samples batches it never marks, which
    /// froze every waterfall in the game for three days.
    ///
    /// `false` for every booth but the **glue create / main-menu scene**. A portrait or
    /// dressing-room bake is a still by design (0130's bake law — it photographs one instant);
    /// the create screen is a live render, and the reference animates a texture transform inside
    /// the per-model-per-frame animate kernel (`0x715f25`-`0x7163bc` over `md+0x74`), which is not
    /// a lane a host opts into.
    pub(super) mat_anim: bool,
}

/// Put the draw gate's marker on a booth child when — and only when — its material went onto the
/// UV / tint lane ([`BoothPart::mat_anim`]). `tick_anim_materials` writes a registered row only for
/// a material some marked, visible part draws, so a registered booth batch that
/// nobody marks stays frozen at its seed with a live table row behind it.
fn mark_mat_anim(child: &mut bevy::ecs::system::EntityCommands<'_>, p: &BoothPart) {
    if p.mat_anim {
        child.insert(benilla_world::doodad_anim::AnimMatPart);
    }
}

/// A batch's translucency twins as the bake sees them — see [`BoothPart::twins`]. Default (both
/// `None`) is "cannot feather", which is what every opaque-bake caller passes.
#[derive(Clone, Default)]
pub(crate) struct BoothTwins {
    pub(crate) blend: Option<Handle<WowModelMaterial>>,
    pub(crate) zfill: Option<Handle<WowModelMaterial>>,
}

/// **The bake's instance-level render properties** — the reference's per-CM2-instance alpha
/// (`model+0x180`) and modulate colour (`model+0x184..0x18c`), which every batch of the model draws
/// through and which every model *attached* to it composes onto (`0x714000`'s recursion).
///
/// One value per bake rather than one per batch, deliberately: that is what it is in the reference,
/// and it is why a booth's riders and cards take it as surely as its body batches do — an item on a
/// ghost is as translucent and as blue as the arm holding it.
///
/// A booth composes at **bake time**, so this is where benilla's one-alpha law (the
/// game declares the translucency, the engine owns the write, the composition and the material
/// swap) is discharged for a booth: the three things the engine owes a translucent instance (the
/// tag's alpha field, the blend-twin swap, and the [`FadeMaterials`] record the depth prime arms
/// off) all happen in [`spawn_booth_model`], together, or not at all.
#[derive(Clone, Copy)]
pub(super) struct BoothInstance {
    /// `model+0x180`. `1.0` = opaque, which is every bake but a ghosted one.
    pub(super) alpha: f32,
}

impl Default for BoothInstance {
    fn default() -> Self {
        Self { alpha: 1.0 }
    }
}

impl BoothInstance {
    /// Does this instance draw translucent — the gate on the whole twin/tag/`FadeMaterials` arm.
    /// Mirrors the reference's own `0 < A < 1` band ([`benilla_world::mesh_tag::translucent`],
    /// which is what the depth prime keys on once the tag is written).
    pub(super) fn feathering(self) -> bool {
        self.alpha < 1.0
    }

    /// The material this instance draws `steady` with, given the batch's twins — the blend twin
    /// while feathering, the steady material otherwise (and the steady one anyway for a batch that
    /// has no twin: a Mod/Mod2x batch reads no alpha, so no swap can feather it — it rides the
    /// shader's identity lerp on the tag alpha instead).
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

    /// The persistent [`FadeMaterials`] record a feathering batch carries, so
    /// [`benilla_world::zfill`]'s depth prime arms on it exactly as it does on a stealthed body in
    /// the world. `None` when the batch cannot feather, or the instance is opaque.
    fn fade_materials(
        self,
        steady: &Handle<WowModelMaterial>,
        twins: &BoothTwins,
    ) -> Option<FadeMaterials> {
        let blend = twins.blend.clone()?;
        self.feathering().then(|| FadeMaterials {
            cutout: steady.clone(),
            blend,
            // A booth is never interior-classified: it has no room around it, and its light is
            // the scene's own authored rig.
            bake_blend: None,
            zfill: twins.zfill.clone(),
        })
    }
}

/// Marks a booth part whose render-alpha `MeshTag` **and `Visibility`** are driven by its own
/// [`MatAnim`](benilla_world::doodad_anim::MatAnim) sample — the booth twin of the world lane's writer.
///
/// The world path's writer is the *visibility authority* (`debug_panel::apply_model_visibility`),
/// which is scoped to `ModelPart` + `GlobalTransform` and culls by distance to the **world** camera.
/// A booth part must never enter that query — it would be far-clipped against a camera it has
/// nothing to do with — so the booth owns this one small writer instead
/// ([`push_booth_mat_alpha`]). Marker-scoped rather than inferred from "has no `ModelPart`", so the
/// two lanes cannot silently overlap.
///
/// Being out of that query means being out of **both** halves of what it does, and the booth owes
/// its parts both: the alpha field *and* the `mat_factor > 0.0` term in its `desired` verdict — the
/// reference's `A <= 0` cull, which fires before the blend mode is ever read
/// (`0x707b3a`–`0x707b5c`). Marking the marker is therefore also a claim of sole `Visibility`
/// authority over the parts that carry it; nothing else in the booth writes theirs.
#[derive(Component)]
pub(super) struct BoothMatAlpha;

/// One bone rider headed into a booth bake ([`PortraitRider`], studio-lit).
pub(super) struct BoothRider {
    pub(super) mesh: Handle<Mesh>,
    pub(super) material: Handle<WowModelMaterial>,
    pub(super) bone: u16,
    pub(super) offset: Vec3,
    /// This rider's translucency twins — an attached model composes onto its parent instance's
    /// alpha (`0x714000`), so a ghost's helm feathers with the head under it. See [`BoothTwins`].
    pub(super) twins: BoothTwins,
}

/// One camera-facing batch headed into a booth bake — the undead/night-elf **eye-glow** (a quad on
/// the eye bone, geoset 302 / geoset 0, additive-fullbright), or an equipped item's / item glow's own
/// such batch. The world path splits these into camera-facing cards ([`benilla_world::billboard`]); a booth
/// is a *separate* camera, so the booth seats the same centred quad under its bone's joint and
/// re-faces it to the booth camera itself ([`face_booth_billboards`]). Its centred quad, material,
/// the bone it rides, where its pivot sits in that joint's frame, and the flag arm.
pub(super) struct BoothBillboardSpec {
    pub(super) mesh: Handle<Mesh>,
    pub(super) material: Handle<WowModelMaterial>,
    pub(super) bone: u16,
    /// The card pivot's offset under that bone's joint (Bevy axes). `ZERO` for a batch of the rigged
    /// model itself — the joint frame already bakes the bone pivot (the 0130 rig identity).
    pub(super) offset: Vec3,
    pub(super) kind: BillboardKind,
    /// The card's translucency twins — a card is a batch of the model like any other, so it takes
    /// the instance alpha too (the reference has ONE instance alpha every batch draws through,
    /// billboard batches included; decision 0836 is the same rule on the world's card lanes).
    pub(super) twins: BoothTwins,
}

/// A spawned booth billboard card: the centred quad seated on its billboard bone's joint (which
/// bakes the bone pivot — the 0130 rig identity), re-faced to THIS booth's camera every frame by
/// [`face_booth_billboards`]. The card despawns with the booth root's joints on the next re-bake.
#[derive(Component)]
pub(super) struct BoothBillboard {
    kind: BillboardKind,
}

impl BoothBillboard {
    /// A booth billboard **frame** — a [`BoothBillboard`] with no `Mesh3d`: an entity whose live
    /// world rotation is a billboard bone's replaced palette basis about this booth's camera, for
    /// the other consumers of that matrix to ride. It draws nothing; only its transform matters.
    ///
    /// The booth twin of [`benilla_world::billboard::BillboardCard::frame_following`], and the same
    /// reasoning (itself 0153's rule): same basis function, same law, one system
    /// — not a second mechanism. Today's caller is the glue booth's equipment riders
    /// ([`super::glue_booth`]): an item model spawns no rig, so nothing else would apply the
    /// replacement to a particle emitter hanging under its billboard bone, and the reference folds
    /// the emitter's record position through exactly this matrix (`0x7190a9`–`0x71910c`, in
    /// `0x718960`).
    ///
    /// The caller owns the frame's **translation** (the billboard bone's pivot in its parent's
    /// frame); [`face_booth_billboards`] writes only the rotation.
    pub(super) fn frame(kind: BillboardKind) -> Self {
        Self { kind }
    }
}

/// One **effect-bearing model** headed into a booth bake, as its particle emitters plus the composed
/// seat: the body bone it rides and its offset in that bone's frame. The booth spawns one host there
/// and owns the emitters off it (the scene-brazier recipe); any geometry the same
/// model carries rides the ordinary [`BoothRider`] / [`BoothBillboardSpec`] lists at the same seat.
///
/// The two lanes that feed it are the two lanes that mirror a dressed look — a live unit's
/// [`crate::portrait::PortraitEffects`] markers, and the glue bake's [`super::PreviewEffects`] —
/// and both mean the same two things: an equipped item's own emitters
/// and the `ItemVisuals` glow a held weapon hangs on its own attachment points.
pub(super) struct BoothEffects {
    pub(super) bone: u16,
    pub(super) offset: Vec3,
    pub(super) emitters: Vec<benilla_assets::ModelEmitter>,
}

/// The in-flight bake's rig: the booth root plus its pending [`RigPose`] buffer (`None` = the
/// boneless bake). Consumers seat on demand-spawned bone anchors via [`Self::anchor`]; the caller
/// MUST [`Self::finish`] once every consumer is seated — the pose buffer only reaches the ECS
/// then, anchors included (they are registered *in* the component).
#[must_use = "call finish(): the pose buffer reaches the booth root only then"]
pub(super) struct BoothRig {
    root: Entity,
    rig: Option<benilla_world::rig_anim::RigPose>,
    /// The palette rig slot this bake allocated (`0` = boneless bake, or the pool was full). It is
    /// also this instance's index into the per-instance TINT table
    /// ([`benilla_world::instance_tint`]) — the same field the vertex stage skins from — so a
    /// caller that wants to colour the whole bake writes it here.
    slot: u16,
}

impl BoothRig {
    /// The anchor entity standing in for `bone` (decision 1355's shape, shared with every world
    /// rig): spawned on first demand, seated at the current composed pose, re-seated by the
    /// compose pass every animated frame. `None` = boneless bake or a bone outside the skeleton —
    /// the consumer misses, exactly as a bad joint index always did.
    pub(super) fn anchor(&mut self, commands: &mut Commands, bone: u16) -> Option<Entity> {
        let root = self.root;
        self.rig.as_mut()?.anchor_for(commands, root, bone)
    }

    /// Whether a rig stands at all — the park gate's "is there anything to park".
    pub(super) fn rigged(&self) -> bool {
        self.rig.is_some()
    }

    /// This bake's palette/tint slot — see [`Self::slot`]. `0` is the world's no-rig sentinel and
    /// is never a real slot, so `0` reads as "this bake cannot be tinted".
    pub(super) fn slot(&self) -> u16 {
        self.slot
    }

    /// Commit the pose buffer onto the booth root — after every consumer is seated. `StageRig`
    /// rides with it: the pair exists together or not at all, so the world-view
    /// parker can never see a booth `RigPose` without the marker that exempts it.
    pub(super) fn finish(self, commands: &mut Commands) {
        if let Some(rig) = self.rig {
            commands.entity(self.root).insert((rig, super::StageRig));
        }
    }
}

/// Strip a booth root's whole rig state — the bake teardown twin of the child despawn. Every
/// empty/re-bake arm must run this: `despawn_related::<Children>` reaps the meshes and anchors,
/// but the pose buffer, the player, the palette slot and the park marker all live ON the root,
/// and a leftover `RigPose`+`AnimationPlayer` pair keeps evaluating (and re-writing palette rows
/// through a leaked `RigSkin` slot) for as long as the booth stands empty.
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

/// Spawn `effects` into a booth bake: one host per effect model, seated on its bone's anchor
/// at the model's offset, with that model's emitters owned by the host. Returns
/// `(emitters, frames)` — how many emitters went up, and how many of them ride a billboard frame.
///
/// `light` is the booth's own light-storage buffer, bound onto every emitter
/// ([`benilla_world::particles::buffer::EffectLightOverride`]) for the same reason a booth part gets a
/// material twin: the world buffer would fog and shade a pane by the world's time of day.
///
/// **Which booths call this is a fidelity split, not a cost one.** The body panes
/// (`<PlayerModel>` widgets — the character window's paper doll, the inspect twin) are *live* in the
/// reference: the widget renders through the per-frame `CSimpleModel` path whose light its ctor
/// stages every frame (`0x76d680`) — the same widget family as the animating glue
/// preview. The round unit-frame portraits are not: `SetPortraitTexture` bakes a
/// fresh instance in ONE draw and caches the texture by GUID/displayId, returning it with no
/// re-render (`0x524f60`). A one-frame draw of a freshly-born particle pool
/// yields nothing, so emitters have no place in a portrait — and a booth that owns emitters must
/// keep its camera awake ([`crate::portrait::Booth::live`]), which is exactly what a still should
/// never need to do.
pub(super) fn spawn_booth_effects(
    commands: &mut Commands,
    rig: &mut BoothRig,
    layer: &RenderLayers,
    light: Option<&bevy::render::render_resource::Buffer>,
    effects: &[BoothEffects],
    // The bake's instance-level properties ([`BoothInstance`]). An effect model attached to a
    // translucent instance composes onto it — see the host spawn below.
    instance: BoothInstance,
) -> (usize, usize) {
    let mut spawned = 0usize;
    let mut frames = 0usize;
    for fx in effects {
        let Some(joint) = rig.anchor(commands, fx.bone) else {
            continue; // bad bone index — bake the body without this model's effects
        };
        // The host stands for the ATTACHED MODEL, which is what makes it the right place to hang
        // the composed alpha: the reference recomposes an attached model's own instance alpha every
        // frame as `child+0x19c = parent+0x19c × child+0x180` (`0x714260`), and the emitter lane
        // then copies that into `emitter+0x1a8` (`0x718960` @`0x719073`) to fold into every
        // particle's alpha. Our particle sim already walks that chain
        // ([`benilla_world::model_fade::ModelAlphas`]) — it had simply never
        // been given a link to walk from a booth, because until a bake could be translucent every
        // booth rider really was opaque. A ghosted select body is the case that made it false.
        //
        // `ModelFade`, not `ModelAlpha`: the game declares the translucency and the engine owns the
        // composition. A booth bake has no ramps to fold, so the walk reads the
        // declared value straight — see [`benilla_world::model_fade::ModelAlphas`].
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
            // A **billboard** bone in the emitter's chain: its palette rows are
            // replaced with the rendering camera's basis about the bone's own pivot, and children
            // multiply onto that, so the origin is `pivot + camBasis·(position − pivot)`. A booth is
            // its own camera, so the frame carrying the replacement is the booth twin of the world
            // lane's card — a mesh-less [`BoothBillboard`], faced by [`face_booth_billboards`] and
            // seated at the pivot under the host. No billboard in the chain (every shipped glow
            // model, most items) → the host itself owns the emitter and the rest pose stands.
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
                    anchor: Some(host), // the cloud anchors at the MODEL; bones compose births only
                    // A booth rider's host is torn down with the bake it belongs to.
                    on_owner_loss: benilla_world::particles::OwnerLoss::Free,
                    // The MODEL this cloud multiplies by: its own host, whose composed
                    // alpha is the bake's instance alpha. `ModelAlphas` reads 1.0 through an
                    // entity carrying no `ModelAlpha`, so an opaque bake costs exactly what it did.
                    alpha: Some(host),
                    // A booth's light is its own buffer (`EffectLightOverride` below), never a
                    // world light node — the world's would fog and shade the pane by the world's
                    // time of day.
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

/// Spawn a booth model's **OWN** particle emitters — the ones authored on its own skeleton, riding
/// its own joints. The twin of [`spawn_booth_effects`], and the distinction is which model the
/// emitters belong to: that one seats a *separate* model's emitters (an equipped item's) on a body
/// bone through a host; this one is the model itself burning — the backdrop scene's braziers, the
/// select pet's flames. Returns `(emitters, frames)` like its twin.
///
/// `root` is the bake root: the fallback owner when a bone has no anchor (boneless bake, bad index),
/// the cloud's anchor, and the parent every emitter is hung under so a re-bake's
/// `despawn_related::<Children>` reaps them.
///
/// **The billboard arm is why this is a shared function and not two loops.** A booth pose drops the
/// camera arms ([`benilla_world::rig_anim::RigPose::without_camera_billboards`] — the world pass
/// would face them at the *world* camera), so unlike a rigged model in the world, a booth's emitter
/// whose chain reaches a billboard bone gets no replacement from its palette and would sit at its
/// rest-pose origin. It needs the same mesh-less [`BoothBillboard`] frame the no-rig lanes use —
/// seated here on the **billboard bone's own joint** at `ZERO`, because that joint already bakes the
/// pivot with the chain above it folded in (the 0130 rig identity, `BoothBillboardSpec::offset`'s
/// rule). Only when that bone has no anchor does it fall back to the rest-pose placement at the
/// pivot under `root`, which is all the no-rig lanes can ever do.
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
                // The joint bakes the pivot → the frame sits at ZERO and only its rotation counts.
                // No joint (boneless bake) → the rest-pose placement, at the pivot under the root.
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
            // The ordinary case: the emitter rides its own bone's joint, in that bone's frame.
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
                // A booth's light is its own buffer (`EffectLightOverride` below), never a world
                // light node — the world's would fog and shade the pane by the world's time of day.
                light_node: None,
                // The bake root is torn down and rebuilt as a whole.
                on_owner_loss: benilla_world::particles::OwnerLoss::Free,
                // A booth bake has no appear/despawn ramp and no self-avatar feather.
                alpha: None,
            },
            // A booth loops its one authored clip forever — the doodad law.
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

/// How the booth bake's `AnimationPlayer` runs. Portraits are a **still** ([`Self::Frozen`] — Stand
/// paused at t = 0, the ref bake); the char-create preview is a **live scene** ([`Self::Loop`] —
/// Stand looping), the one case where the ref screen itself animates.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BoothMotion {
    Frozen,
    Loop,
}

/// Spawn a booth bake under `root` on the booth's layer — the ref mechanism (instance `0x707400`,
/// pose `0x7121a0`): a **fresh throwaway instance posed at Stand**, never the unit's live world
/// pose.
///
/// With a rig (skeleton + inverse bindposes; every M2 display), the booth builds a collapsed
/// [`RigPose`](benilla_world::rig_anim::RigPose) buffer (no joint entities),
/// draws each part's **skinned** twin against its palette slot, seats riders on their bone's
/// anchor, and arms the model's own Stand (anim id 0 through its baked resolution — the ref's
/// loader-idle seed): `motion` decides whether that Stand is **frozen at t = 0** (a portrait
/// still) or **looping** (the live glue scenes/preview). (The ref's own
/// sampling clock is the one unsettled, inferred point — t≈0 vs live phase; a frozen t=0 is
/// inside its envelope either way.) Without a rig (boneless / WMO-display / rig not built), the
/// static bind-pose bake: parts at identity, riders dropped (no bones to seat them on).
///
/// Returns the [`BoothRig`] handle — seat any remaining consumers on it (the effect hosts, the
/// glue scene's emitters), then `finish()` it.
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
    // Per-hand weapon grip `[right, left]` — hold that hand's `HandsClosed` finger pose because a weapon
    // occupies its attach point (the paperdoll rule, `0x5059a0`). The glue
    // preview and the body panes both resolve it from **attachment occupancy**, which is the whole
    // rule (`crate::portrait::hand_grip`); the round portraits pass `[false, false]` because the
    // reference's portrait bake never arms a grip at all, not because they sheath anything.
    grip: [bool; 2],
    // Character billboard batches (the undead/night-elf eye-glow) — seated on their billboard bone's
    // joint and re-faced to the booth camera by [`face_booth_billboards`]. Needs the rig (no bones =
    // no eye bone); the boneless bake below drops them. `&[]` for booths that dress none.
    billboards: &[BoothBillboardSpec],
    // The bake's instance-level render properties — see [`BoothInstance`]. `default()` (opaque) for
    // every booth but a ghosted character select.
    instance: BoothInstance,
) -> BoothRig {
    // A re-bake must not inherit the previous model's animation state on the shared root — nor a
    // stale pose buffer (its anchors died with the child despawn), nor the previous rig's palette
    // slot (`RigSkin`'s on_replace hook frees it; the boneless bake below has no new one).
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
            // The authored material alpha reaches the boneless bake too — it is a property of the
            // batch, not of the rig. The INSTANCE alpha multiplies into it, the reference's
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
    // The collapsed pose buffer (replaying 0724/1365 for the last entity-joint
    // rig lane): bind-pose composed, evaluated in place by the 0712 evaluator off the player
    // below. `without_camera_billboards` because a booth is not the world camera — see its doc;
    // the booth's cards counter-rotate to their own camera instead.
    let mut pose =
        benilla_world::rig_anim::RigPose::new(root, skeleton).without_camera_billboards();
    // The owned palette rig: the booth's skinned parts tag this slot; the world
    // pass writes its rows from the composed pose, and the booth's studio light buffer mirrors
    // the palette region (`rig_palette::RigPaletteMirrors`), so the booth camera sees the pose.
    let rig_slot = benilla_world::rig_palette::RigSkin::allocate_bones(
        palettes,
        skeleton.joints.len() as u32,
        ibp.clone(),
    )
    .map_or(0, |rig| {
        let slot = rig.slot;
        commands.entity(root).insert(rig);
        // Booth materials bind a STUDIO light buffer, not the shared one — route this
        // rig's rows to the registered mirrors too.
        palettes.mark_mirrored(slot);
        slot
    });
    // The model's global-sequence bone channels, by motion:
    // - **Loop** (the glue scenes + the create/select character): LIVE, on the world's own
    //   clock-driven sampler — the login gate's fires flicker, the Tauren windmill turns, the
    //   character blinks.
    // - **Frozen** (portrait stills): frozen at t = 0 — for the eyelid that is scale 0 there (lid
    //   retracted, eye OPEN), matching "Stand frozen at t = 0" (a still must hold the open frame).
    //   Stand keys no global-sequence bone, so the paused player never overwrites the freeze.
    //   Without it the eyelid sits at identity scale — eye shut.
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
                    // `locals` are seeded at bind pose, so the untouched properties keep the
                    // rest transform — the same base the joint-entity form built from.
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
        // The batch's authored material alpha, sampled onto the render-alpha tag field — the same
        // `MatAnim` the world lanes use, in its self-driving form (`drives_tag`: nothing else writes
        // a booth part's alpha). The rig field rides the same tag, so compose rather than overwrite;
        // `alpha_bits` floors a true zero at ≈0 so the shader's whole-payload-`0` *untagged ⇒
        // opaque* sentinel can't fire on a batch the artist authored invisible.
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
        // The persistent twin record: while this instance draws translucent, `benilla_world::zfill`
        // keeps a colour-masked, z-writing child on it, so overlapping limbs stay ONE blended
        // layer instead of compounding 0.5 over 0.5 (the reference's `M2UseZFill`).
        if let Some(fm) = instance.fade_materials(&p.material, &p.twins) {
            child.insert(fm);
        }
        if use_rig {
            child.insert(benilla_world::rig_palette::RigPart(root));
            // **A skinned booth part is never frustum-culled** — the same law the world's dress
            // path states (`entities::attach::dress`, 0648/1270/1473), and for a sharper reason
            // here. The part's vertices are moved by the GPU joint palette; the only bound Bevy
            // has for them is `calculate_bounds`' box over the mesh's own **bind-pose** vertices,
            // and this camera is the artist's *portrait* camera — calibrated against the model in
            // its **Stand** pose. On any model whose rest pose is not its Stand the two are
            // nowhere near each other, so the bind box falls outside the frustum and every batch
            // is culled while the posed geometry is dead centre of frame. `Creature\CarrionBird`
            // is the case: bind-pose z tops out at 1.19, its Stand box runs 0.54..6.23, and the
            // authored camera sits at z = 3.03 looking at z = 2.97 — nothing drew, and the
            // portrait baked the empty booth (report B92). `Creature\Worm` the
            // same. There is no cull to lose: a booth holds one model that its camera was
            // authored to frame.
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
            continue; // bad bone index — bake the body without this rider
        };
        let mut child = commands.spawn((
            Mesh3d(r.mesh.clone()),
            MeshMaterial3d(instance.material(&r.material, &r.twins)),
            Transform::from_translation(r.offset),
            layer.clone(),
            ChildOf(anchor),
        ));
        // An attached model composes onto its parent instance's colour and alpha (the reference's
        // `0x714000` recursion; decision 0820's "an item's parts carry the wearer's instance
        // bits"). Untagged means opaque + slot 0, so the tag only goes up when there is something
        // to say — a translucent instance, or a tint slot to borrow.
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
    // The camera-facing batches (the eye-glow, an item's gem, a glow model's quad): seat the centred
    // quad under its bone's anchor at the spec's offset — zero for the body's own batches, whose
    // anchor frame already bakes the bone pivot (the 0130 rig identity), so the quad lands at the
    // eye — and tag it for [`face_booth_billboards`], which rewrites its rotation to the booth
    // camera each frame. The rotation the anchor carries here (its Stand pose) is countered there.
    // A bone the rig lacks drops the card, like a rider.
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
    // Arm Stand and freeze: the player is configured *before* insertion (plain component data),
    // so the pose lands with the first animation pass — no play-after-spawn ordering dance.
    // `ModelAnimations` goes up beside it: the 0712 evaluator reads the player THROUGH the baked
    // pose source (`(&AnimationPlayer, &ModelAnimations, &mut RigPose)`) — no `AnimatedBy`
    // targets exist to route bevy's own sweep here, which is the point.
    if let Some(anims) = anims {
        let stand = catalog.map_or(0, |c| anims.resolve(0, c).id);
        if let Some(clip) = anims.find(stand) {
            let mut player = AnimationPlayer::default();
            // A portrait is a still (Stand paused at t = 0); the char-create preview is a live scene
            // (Stand looping) — the one case the ref screen itself animates.
            match motion {
                BoothMotion::Frozen => {
                    player.play(clip.node).pause();
                }
                BoothMotion::Loop => {
                    player.play(clip.node).repeat();
                }
            }
            // Close each hand that holds a weapon: play its `HandsClosed` finger overlay *over* Stand
            // (masked to that hand's finger subtree, weight-dominant), held with `.repeat()` because it
            // is a single-key clamp pose — the same arming the live [`drive_hand_grip`] does, applied
            // once at spawn since a booth bake's grip never changes after it's built.
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

/// The ids `PlayerModel:SetRotation` chooses between — read off the reference's own name table
/// (`0x6143b6 mov eax,[ecx*4+0x8686a8]`: 0 `Stand`, 11 `ShuffleLeft`, 12 `ShuffleRight`), not
/// from DBC lore.
const STAND: u16 = 0;
const SHUFFLE_LEFT: u16 = 11;
const SHUFFLE_RIGHT: u16 = 12;

/// **Which sequence a facing change arms** — the reference's own direction test, stated once
/// (`0x505bb0`).
///
/// The `fcomp` at `0x505bce` compares the **current** facing (in ST(0)) against the argument, and
/// the two branches read out as: current **<** angle ⇒ `0xc` **ShuffleRight**, current **>** angle
/// ⇒ `0xb` **ShuffleLeft**. Equal — and NaN, which falls the same way — arms `0` **Stand**, and
/// that is an *active* play, not a no-op: it is why `Model:SetSequence` cannot stick on one of
/// these panes.
///
/// **This port was first built with this pair inverted.** The direction was wrong in exactly the
/// way nothing local can catch — the doll still stepped, just into its turn instead of with it.
///
/// It runs on the **Lua-facing scalar** — the very value the reference hands `SetRotation` — so
/// the branch transfers with no sign work, and no reasoning about which way a Bevy `+Y` spin goes
/// has to be right for the feet to match the turn.
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
/// (`0x42c010` is a millisecond clock, so `+0x64` is +100 ms), drained by `0x505c50`'s per-paint
/// timer.
///
/// The arm is **unconditional** — every one of `0x505bb0`'s three early exits jumps to `0x505c28`,
/// where the flag and the deadline are written, so no path through it skips them.
/// A held rotate arrow rewrites the facing every frame (the pane's `OnUpdate`,
/// `ROTATIONS_PER_SECOND`), so the deadline is pushed forward every frame and the doll steps
/// continuously until the button comes up. Taking `spun` before the expiry check below reproduces
/// the reference's second guard for free: `[+0x3e8]` is a one-paint "a rotation happened since the
/// last paint" latch, and a paint preceded by a `SetRotation` can never expire.
const SHUFFLE_HOLD_SECS: f64 = 0.100;

/// How much of a cross-fade's window is **still to run**, `1 → 0` — the `t` the client recomputes
/// inline from the live descriptor and clock on every arm and every frame (`(blendEnd − now) ·
/// blendRate`), never a cached weight. Named because two places must agree on it exactly: the
/// per-frame weights, and the half-blend refusal that decides whether an arm re-seeds at all.
fn fade_frac(fade: &super::Fade, now: f64) -> f32 {
    ((fade.until - now) / f64::from(fade.span)) as f32
}

/// Arm one `AnimationData` id on a booth root the way the reference's turn does — every one of its
/// plays is `0x7121a0(bone -1, id, variation -1, offset 0, rate 1.0f, blend 1, primary 1)`, and
/// all three of those trailing arguments show:
///
/// - **variation `-1`** — a freshly *rolled* frequency-weighted variation, not the head. HumanMale
///   authors four Stands (frequencies 14199 / 2184 / 2184 / 14199), and the reference re-rolls on
///   every arm including the one its expiry timer fires, so a doll does not come back from two
///   turns into the same idle twice.
/// - **offset `0`** — from the top of the clip, not from wherever it happened to be.
/// - **blend `1`** — cross-faded out of the outgoing pose over the **incoming** clip's own
///   `M2Sequence.blendTime` ([`super::Fade`]). On HumanMale that is 0.25 s entering a shuffle and
///   **0.5 s** coming back to Stand (`benilla-extract m2seq`), and that half-second is the whole of
///   B321: the release used to cut from a mid-stride shuffle straight onto Stand in one frame.
///
/// Returns whether it armed. A display that does not author the id at all — a pet's model, mostly,
/// for the shuffles — is left standing rather than stepping whatever the resolver walked away to,
/// so `false` says "there is no turn to run here" and no expiry is scheduled for it. (The old node
/// compare this replaces only caught the case where the fallback landed on Stand's *head*
/// variation, which is not where a frequency roll usually lands.)
fn arm_turn(
    player: &mut AnimationPlayer,
    anims: &benilla_assets::ModelAnimations,
    catalog: &benilla_formats::AnimDataCatalog,
    rng: &mut benilla_assets::AnimRng,
    turn: &mut super::Turn,
    id: u16,
    now: f64,
) -> bool {
    // What this arm fades *out of*: whatever the last one left playing, or — before the first turn
    // of this bake — the Stand the bake itself armed, which is the head variation
    // (`spawn_booth_model` plays `find(resolve(0))`).
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
    // **The half-blend refusal** (`0x7125c9`/`0x7125d4`). A blend already running
    // with λ > 0.5 — more than half its window still to run — is NOT re-seeded: the client keeps
    // the old secondary on its old window, and the clip that was primary is simply dropped. Read
    // as a rule: *a pose that never got past half weight is not worth fading out.* Equality takes
    // the snapshot, and the strictness is real — the f32 lands exactly on 0.5 at `remaining =
    // span/2`, so the boundary falls on the snapshot side.
    //
    // Reachable in ordinary use, which is why it is here: Stand's blend is 500 ms, so **a second
    // nudge of the arrow within 250 ms of the last release** meets it — a repeated-nudge cadence,
    // not an exotic input. (λ against 0.5 rather than the fraction against ½ because the law is
    // stated on λ; at our amplitude of 1.0 the two are the same test, `smoothstep` being strictly
    // increasing with `smoothstep(½) = ½`.)
    let refused = turn
        .fade
        .is_some_and(|f| crate::creature_anim::select::blend_lambda(fade_frac(&f, now)) > 0.5);
    if refused {
        // The outgoing primary goes nowhere — not into the secondary, which keeps the older pose
        // it was already fading, at the older λ. Only this clip is dropped.
        if let Some(out) = outgoing.filter(|o| *o != node) {
            player.stop(out);
        }
    } else if let Some(old) = turn.fade.take() {
        // The one secondary slot: a fade still in flight is displaced here, and its pose stops
        // being drawn at all — except when it is the node we are about to (re)play, which a
        // reversal *back* inside a running window makes it (hold left, flick right, flick left
        // again). The second clause holds the invariant rather than a live case: an arm always
        // leaves `fade.node` and `playing` distinct, so the displaced fade is never the pose the
        // new one starts from.
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
        // Explicit, not defaulted: `play` is idempotent on a live node, so a re-arm of a node
        // some earlier turn left running would otherwise keep that turn's clock and weight.
        let active = player.play(node);
        active.set_repeat(if looping {
            RepeatAnimation::Forever
        } else {
            RepeatAnimation::Never
        });
        active.set_speed(1.0);
        active.seek_to(0.0);
        // λ = 1 on a blended arm's first frame, so the incoming contributes nothing yet — which is
        // exactly the point: the pose does not move on the frame the arm lands.
        active.set_weight(if fade.is_some() { 0.0 } else { 1.0 });
    }
    if fade.is_none() {
        // Either nothing was playing, or the sequence authors `blendTime = 0` — an instant cut,
        // the client's own `[blk+0xd0] = -1` no-blend path.
        if let Some(out) = outgoing.filter(|o| *o != node) {
            player.stop(out);
        }
    }
    turn.playing = Some(node);
    turn.fade = fade;
    true
}

/// **Step the doll's feet round when it turns, and settle out of the step** (decisions 1559 +
/// 1565, director reports B313/B321).
///
/// The reference's rotate arrows do not spin the model on the spot: `Model:SetRotation` queues a
/// turn-in-place shuffle by direction *and then* writes the facing. Ours wrote only the facing
/// (0638's bake yaw), so the doll pivoted like a turntable.
///
/// Both halves of that turn are **blended** arms ([`arm_turn`]), which is the second half of the
/// same law and the fix for B321: the reference's 100 ms expiry does not drop the shuffle, it
/// cross-fades Stand in over half a second while the shuffle keeps stepping underneath and fades
/// out. Ours cut, and a cut out of a mid-stride pose is the "snap back to the stop pose" the
/// director saw. Nothing here stops a clock early: the client's `rep movsd` copies the outgoing
/// track's clock, not a pose, so the shuffle goes on stepping underneath the whole fade — a
/// half-second, on HumanMale, which is a further whole loop of it.
///
/// Body panes only ([`Booth::live`]), which is the same set the reference's turn machinery sits
/// on. The doll cannot clack while it steps: footstep keys are fired by
/// `creature_anim::events::fire_anim_events`, whose query demands an `AnimDriver`, and a booth
/// root has never carried one.
pub(super) fn drive_booth_turn(
    time: Res<Time<bevy::time::Real>>,
    mut booths: ResMut<super::Booths>,
    anim_data: Option<Res<crate::creature_anim::AnimData>>,
    mut rigs: Query<(&mut AnimationPlayer, &benilla_assets::ModelAnimations)>,
    // The variation roll's state — the reference passes variation `-1` to `0x7121a0`, which is
    // "frequency-weighted random" off the CRT LCG (`0x71249a call 0x7400e5`). Same generator the
    // world lane's picks use, so a doll and a world unit roll their idles alike.
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
            // No rig in this booth (empty, or a boneless display): a turn in flight when the bake
            // went away has nothing left to step or fade, and its nodes name a player that is gone.
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

/// One booth's turn, for one frame — [`drive_booth_turn`]'s whole rule, with the ECS shelled off so
/// it can be stepped directly at chosen times.
fn step_turn(
    player: &mut AnimationPlayer,
    anims: &benilla_assets::ModelAnimations,
    catalog: &benilla_formats::AnimDataCatalog,
    rng: &mut benilla_assets::AnimRng,
    turn: &mut super::Turn,
    now: f64,
) {
    // Arm: a rotation this frame plays its direction's sequence and re-arms the expiry. The
    // reference's test is an id compare against what sits on the root bone (`0x712090`), so a HELD
    // direction arms once and lets the 500 ms clip loop, while a reversal is a fresh arm — the
    // opposite id is by definition not the armed one — restarted from `t = 0`.
    if let Some(want) = turn.spun.take() {
        let armed = turn.shuffle.map_or(STAND, |(id, _)| id);
        if want == armed {
            // Same direction still held: only the deadline moves. (An equal facing sent to an
            // already-standing doll lands here too, and correctly does nothing — the reference's
            // compare skips the play for exactly the same reason.)
            if let Some((_, until)) = turn.shuffle.as_mut() {
                *until = now + SHUFFLE_HOLD_SECS;
            }
        } else if arm_turn(player, anims, catalog, rng, turn, want, now) {
            // Equal facings — and NaN — arm Stand outright (`0x505c23`), which is an *active*
            // play, not a no-op; nothing is stepping after it, so it schedules no expiry.
            turn.shuffle = (want != STAND).then_some((want, now + SHUFFLE_HOLD_SECS));
        }
    }
    // Expire: the arrow came up (nothing re-armed the deadline) — back to Stand, re-rolled and
    // blended, the reference's `0x505c98`. Unreachable in the same frame as an arm, which always
    // leaves the deadline a full [`SHUFFLE_HOLD_SECS`] out.
    if let Some((_, until)) = turn.shuffle {
        if now > until {
            arm_turn(player, anims, catalog, rng, turn, STAND, now);
            turn.shuffle = None;
        }
    }
    // Advance the cross-fade. λ = smoothstep of the window STILL TO RUN
    // ([`crate::creature_anim::select::blend_lambda`], the client's `0x714880` kernel) and it
    // weights the OUTGOING pose; the incoming takes `1 − λ`. The pair sums to 1 every frame, so
    // under the evaluator's normalized weighted fold the mix is exactly the client's
    // `out = primary + (secondary − primary)·λ` — and anything else on the rig (the hand-grip
    // finger overlay the bake holds) keeps the relative weight it had throughout.
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

/// Re-face each booth billboard card ([`BoothBillboard`]) to its booth's camera — the booth twin of
/// the world's [`benilla_world::billboard::face_billboards`]. Each booth owns one camera, matched here by
/// their shared render layer. The card is a child of its billboard bone's joint, so we set its
/// **local** rotation to counter the joint's world rotation and land the world rotation on the
/// camera basis; the joint carries translation/scale (the eye pivot, the booth/character scale). The
/// joint pose is read a frame stale (its global is last propagate's), invisible on the near-static
/// Stand loop the booth runs — the same latency budget the paper-doll/portrait stills already accept.
pub(super) fn face_booth_billboards(
    cams: Query<(&GlobalTransform, &RenderLayers, &Camera), With<super::BoothCam>>,
    joints: Query<&GlobalTransform>,
    mut cards: Query<(&BoothBillboard, &ChildOf, &RenderLayers, &mut Transform)>,
) {
    for (card, child_of, layers, mut tf) in &mut cards {
        let Some((cam, _, _)) = cams
            .iter()
            // A sleeping camera renders nothing — leave its cards be (the booth park); they
            // re-face on the wake window's first frame, before anything samples the target.
            .find(|(_, l, c)| c.is_active && l.intersects(layers))
        else {
            continue; // …or the card's booth camera isn't up at all (booth torn down)
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

/// Resolve each booth part's sampled material alpha — **cull it at `A <= 0`, dim it otherwise** —
/// the booth's own tiny twin of the world visibility authority (see [`BoothMatAlpha`] for why the
/// world one can't serve here).
///
/// `doodad_anim::sample_mat_anim` already ticks **every** `MatAnim` in the world, booth parts
/// included, so this only spends the sampled value. Two ways, exactly as
/// `model_render::visibility` spends it on a world part:
///
/// - **`A <= 0` → `Hidden`.** The reference culls the batch before it looks at the blend mode, so a
///   zero is a *disappearance*, not a fade to nothing — and only a `Visibility` write can say that.
///   The tag alpha reaches the shader's blend source, which an Opaque draw ignores outright
///   (`wow_model.wgsl`: "For steady cutout/opaque draws blend is off so this is ignored"), so a
///   batch the artist keyed off in this sequence would otherwise draw **solid**. That is not an
///   edge case: `alphascan` counts 56 of 420 creature models authoring per-sequence batch
///   visibility, and the shipped one that stands in this booth is the Voidwalker, whose two
///   shoulder props are keyed to zero for the whole of Stand and hung in the air without this.
///   `Inherited` rather than `Visible`, so a hidden bake root still takes its parts with it.
/// - **Otherwise → the alpha field.** `with_alpha` writes that field alone, so the rig slot (and the
///   probe slot the rig lane reads from the same tag) ride through untouched.
///
/// Both are write-on-change, so the overwhelmingly common case — an authored *constant* like
/// UI_Tauren's 0.55 vignette — costs two compares per part per frame and never re-batches.
///
/// `Visibility` is required, not optional, and that is safe by construction: every site that adds
/// [`BoothMatAlpha`] spawns the part with a `Mesh3d`, which requires `Visibility` — and a part
/// without one is not a booth draw at all. Required rather than `Option`, because an optional match
/// would quietly ship the dimming half alone again, which is the whole defect this system had.
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
        // The cull, the same term the world authority ANDs into its verdict as `mat_factor > 0.0`.
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

    /// A constant authored alpha, the shape every glue scene actually uses (UI_Tauren's vignette is
    /// `0.55..0.55`, its ground shadow `0.99..0.99`).
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

    /// The writer moves the sampled alpha onto the tag **and leaves the rig slot alone** — the whole
    /// reason it goes through `with_alpha` rather than assigning the field. A booth part is skinned,
    /// so a writer that clobbered bits 19..=29 would silently unbind its palette.
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
                // Mirrors production: both marker sites spawn a `Mesh3d`, which *requires*
                // `Visibility` — the writer's query takes it unconditionally, so a fixture without
                // one is not a booth part and would silently fall out.
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

    /// The `A <= 0` cull — the half the booth twin was missing, and the reason the Voidwalker
    /// stood in the char-select booth with two shoulder props hung in the air. The reference culls
    /// the batch before it reads the blend mode (`0x707b3a`–`0x707b5c`), and the world
    /// lane says so as `mat_factor > 0.0` in its `desired` verdict
    /// (`model_render::visibility`); a tag write alone cannot express it, because an Opaque draw
    /// never looks at the blend source the tag alpha feeds. Both directions, in one run: a zeroed
    /// batch goes `Hidden`, a live one stays drawable, and the writer keeps up when the sample moves.
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

        // Not a one-shot verdict: the sample is per frame, so a batch keyed back on comes back.
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

    /// **The booth twin of the equipped-item emitter's billboard frame** (carried to
    /// the select screen). Same real numbers as
    /// `billboard::tests::an_item_emitters_billboard_frame_puts_it_behind_the_pivot`
    /// (`LShoulder_Mail_PVPAlliance_C_01`: billboard bone 1 pivot `(-0.012, 0.162, -0.060)`, sparkle
    /// emitter `(-0.252, 0.178, -0.046)`, raw WoW model space), but through THIS system — and here the
    /// frame is a **child**: the item's host sits under a posed body joint, so the law holds only if
    /// the parent's rotation is cancelled rather than inherited. That cancellation is the one thing
    /// that could silently be wrong on the booth path, so it is what the joint pose below is for.
    ///
    /// Asserted: the frame sits AT the pivot; the emitter's composed origin is 0.24 yd along the
    /// **booth** camera's view axis, away from the eye, with only centimetres off it; and it FOLLOWS
    /// that camera — the half a rest-pose placement cannot do.
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
                // `Camera` because the facer skips a sleeping camera (the booth park);
                // the default is active — the state a rendering booth is in.
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

        // The system writes the frame's LOCAL transform; composing it onto the host is what
        // propagation does in PostUpdate, before the particle sim reads the result.
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

        // Move the booth camera (the preview's yaw drag does exactly this relative to the model):
        // the offset must follow it. A rest-pose placement would return the same vector twice.
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

    /// **A skinned booth part is spawned exempt from the frustum test** — decision 1577, report B92.
    ///
    /// The part's vertices live in the GPU joint palette; the only bound Bevy can build for it is
    /// `calculate_bounds`' box over the mesh's own **bind-pose** vertices, and the booth camera is
    /// the artist's portrait camera, aimed at the model's **Stand**. `Creature\CarrionBird` bakes
    /// a bind box topping out at z = 1.19 while that camera sits at z = 3.03 looking at z = 2.97:
    /// every batch culled, an opaque-black portrait, and the posed bird dead centre of a frustum
    /// nothing was ever tested against. The world's dress path states the same exemption
    /// (`entities::attach::dress`); this is the booth half, and it is what the marker guards.
    ///
    /// The unskinned twin is the control: a static part draws its bind-pose mesh *at* bind pose, so
    /// its own box is the truth and it keeps the ordinary test.
    #[test]
    fn a_skinned_booth_part_is_never_frustum_culled() {
        let mut app = App::new();
        app.init_resource::<benilla_world::rig_palette::RigPalettes>();
        app.init_resource::<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>();

        // A two-bone rest skeleton — enough for `RigPose::new` and a real palette slot.
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

    /// An unmarked part is not ours to write: the world lane's parts carry `MatAnim` too, and their
    /// alpha is composed by the visibility authority against the fade and the interior classifier.
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

    /// **The rotate arrows arm the shuffle the model turns toward** (1559), and the pair is
    /// the one read off the `fcomp` at `0x505bce` — *not* the inverted pair this port was first
    /// built on. Current facing **<** the new angle ⇒ `0xc` ShuffleRight; **>** ⇒ `0xb`
    /// ShuffleLeft; equal (and NaN, which the compare's unordered flags send the same way) ⇒ `0`
    /// Stand, an active arm rather than a no-op.
    ///
    /// The pane's own Lua is what makes the mapping checkable end to end, and it is why an
    /// inversion cannot be caught by reading either side alone: the reference uses **opposite
    /// sign conventions** in its two callers, and both must still land on the foot the model
    /// turns onto. The held arrows (`BenillaPaperDollModel_OnUpdate`) have held-LEFT *add*; the
    /// click helpers (`BenillaPaperDollModel_RotateLeft`) have left *subtract*.
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
        // Held: left ADDS, so a held left arrow steps ShuffleRight — the reference's own pairing,
        // and the one that reads backwards until you have both halves in front of you.
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
        // Clicked: left SUBTRACTS, so the same arrow lands on the other shuffle. Both are the
        // reference's, quoted not re-derived.
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

    /// The three ids a turn ever arms, with HumanMale's **real** authored numbers
    /// (`benilla-extract m2seq Character\\Human\\Male\\HumanMale.m2`): the shuffles blend in over
    /// 0.25 s and loop for 0.5 s; Stand blends in over **0.5 s** and has four variations. Two
    /// Stands here, weighted so the frequency roll cannot land on the head — that is what makes
    /// "did it re-roll, or did it just take `find`'s head?" a decidable question below.
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
                clip(STAND, 1, 0.5, 1),         // the head — all but unreachable by the roll
                clip(STAND, 2, 0.5, 32766),     // where a roll lands
                clip(SHUFFLE_LEFT, 3, 0.25, 0), // the reference's 32767, i.e. the only variation
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

    /// The bake's own arm: Stand's **head** variation, looping — what `spawn_booth_model` leaves
    /// running, and therefore the pose the first turn of a session fades out of.
    fn baked() -> AnimationPlayer {
        let mut player = AnimationPlayer::default();
        player.play(node(1)).repeat();
        player
    }

    fn weight(player: &AnimationPlayer, n: usize) -> Option<f32> {
        player.animation(node(n)).map(|a| a.weight())
    }

    /// **The release settles instead of snapping** — B321, and the half of the reference's turn
    /// that decision 1559 shipped without.
    ///
    /// Every arm `0x505bb0`/`0x505c50` makes carries `blendFlag = 1`, so both ends of a turn are
    /// cross-fades, and each runs for the **incoming** clip's own `blendTime`: 0.25 s into the
    /// shuffle, 0.5 s back out of it. The frame an arm lands on must not move the doll at all
    /// (λ = 1 ⇒ the outgoing pose, whole), and the weights must sum to 1 throughout so that the
    /// evaluator's normalized fold lands exactly on the client's
    /// `out = primary + (secondary − primary)·λ` — and so the hand-grip overlay riding the same rig
    /// keeps its share.
    #[test]
    fn a_turn_blends_both_ways_over_the_incoming_clips_own_time() {
        let (anims, catalog) = (
            turning_model(),
            benilla_formats::AnimDataCatalog::from_rows([]),
        );
        let (mut player, mut rng) = (baked(), benilla_assets::AnimRng::default());
        let mut turn = super::super::Turn {
            // Arrow down. The shuffle is armed but contributes NOTHING yet.
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

        // Mid-blend: λ = smoothstep of the window still to run, on the SHUFFLE's 0.25 s.
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

        // Arrow up. 100 ms later the expiry arms Stand — and THIS frame must not move the doll.
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

        // …and it runs for STAND's 0.5 s, not the shuffle's 0.25 s: still blending at 0.3 s in.
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

    /// **The half-blend refusal, both legs**. One secondary slot, as the client
    /// keeps one — but an arm only *takes* it when the running blend is at or past halfway. Inside
    /// the first half, `0x7125d4` refuses: the older pose keeps fading on its own untouched window,
    /// and the clip that was primary is dropped outright rather than fading out of a weight it
    /// never reached.
    #[test]
    fn a_reversal_is_refused_inside_the_half_blend_and_displaces_it_after() {
        let (anims, catalog) = (
            turning_model(),
            benilla_formats::AnimDataCatalog::from_rows([]),
        );

        // Reverse at 0.1 s of a 0.25 s blend — 60% still to run, λ = 0.648. REFUSED.
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

        // Reverse at 0.2 s instead — 20% still to run, λ = 0.104. The slot is taken.
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

    /// The refusal's **reachable** case, and the one that decided it was worth building: Stand
    /// blends in over 0.5 s, so a second nudge of the arrow within 250 ms of the last release meets
    /// a blend that is more than half to run. Without the guard those stack — each nudge would fade
    /// out a Stand that had barely faded in. With it, the doll goes on fading out of the shuffle it
    /// was actually in.
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

        // Nudged again 100 ms later — 80% of Stand's blend still to run, λ = 0.896. REFUSED.
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

    /// A display that does not author the shuffles at all — most pet models — is left **standing**,
    /// not stepping whatever the id resolver walked away to. Nothing is armed, so nothing expires.
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

    /// Three distinguishable handles — the assertions are about *which* of them the instance
    /// picks, so they only need to be unequal.
    fn handles() -> (
        Handle<WowModelMaterial>,
        Handle<WowModelMaterial>,
        Handle<WowModelMaterial>,
    ) {
        let assets = Assets::<WowModelMaterial>::default();
        let one = || assets.reserve_handle();
        (one(), one(), one())
    }

    /// The three things the engine owes a translucent instance happen **together, or not at all**
    /// (decision 1164's law, discharged at bake time): the blend-twin swap, and the persistent
    /// [`FadeMaterials`] record the depth prime arms off. An opaque bake gets neither — which is
    /// what keeps every existing booth bit-for-bit unchanged by the ghost lane.
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

    /// A ghosted bake takes the blend twin and carries the record — with the **zfill** twin in it,
    /// which is the whole reason a 0.5 body does not compound `0.5` over `0.5` where an arm crosses
    /// the torso (the reference's `M2UseZFill`).
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

    /// The emitters an attached effect model carries compose onto the instance too — the reference
    /// recomposes `child+0x19c = parent+0x19c × child+0x180` every frame (`0x714260`) and copies it
    /// into `emitter+0x1a8`. Pinned as the *link* rather than the arithmetic (the fold itself is
    /// `benilla_world`'s and already tested there): what this asserts is that a feathering bake
    /// gives its effect host a `ModelAlpha` at all, which is the half that was missing — a ghost's
    /// wisps drew at full strength over a body at 0.5 until it existed.
    #[test]
    fn a_feathering_bake_hands_its_effect_host_an_alpha_to_compose() {
        assert!(BoothInstance { alpha: 0.5 }.feathering());
        assert!(
            !BoothInstance::default().feathering(),
            "and an opaque bake hands it none, so every existing booth is untouched"
        );
    }

    /// A batch that cannot feather keeps its steady material even while the instance is
    /// translucent, and arms no prime — the reference's own twin gate, and the Mod/Mod2x rule
    /// (their blend equation reads no alpha, so no material swap can feather them; the shader's
    /// identity lerp on the tag alpha does it instead).
    #[test]
    fn a_twinless_batch_stays_steady_even_while_the_instance_feathers() {
        let (steady, _, _) = handles();
        let none = BoothTwins::default();
        let ghost = BoothInstance { alpha: 0.5 };
        assert_eq!(ghost.material(&steady, &none), steady);
        assert!(ghost.fade_materials(&steady, &none).is_none());
    }
}
