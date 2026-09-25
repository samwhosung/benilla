//! The doodad animation host: placed M2 doodads (ADT MDDF and WMO MODD props) animate.
//!
//! The reference arms a doodad at load (bone 0, animation id 0, `linkFlag=1`) and re-arms it every
//! play-window with a fresh weighted variation: the watchdog `0x719370` fires on `now ≥ windowHi`
//! and runs the callback `0x6951b0` that `0x695100` installed at `[model+0x70]`, which re-arms
//! with `variationIdx = -1`. Global sequences loop with no arming. The walk `0x7074b0` advances
//! only the models the doodad drain `0x683f80` links each frame from the drawn and faded set, and
//! sampling is clock-indexed (`cursor = clock − startOffset`), so pausing drifts nothing.

use bevy::animation::graph::AnimationNodeIndex;
use bevy::app::AnimationSystems;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;

use benilla_assets::{AnimClip, M2Model, ModelAnimations, ModelSkeleton};

use crate::rig_anim::{AnimParked, GlobalSeqDrive, RigPose};
use crate::vis_chain::VisChainOnly;

mod lazy;
mod mat_anim;
pub(crate) use lazy::{LazyRig, SkinnedTwin};
use mat_anim::tick_anim_materials;
pub use mat_anim::{
    playing_seq, register_entity_uv, register_fx_uv, register_tint, sample_mat_anim, AnimMatPart,
    MatAnim, TintAnimMaterials, TintLoop, UvAnimMaterials, UvLoops,
};
pub(crate) use mat_anim::{register_uv, UvLoop};

/// What a placed doodad model animates, decided per model at spawn.
pub enum DoodadAnimTier<'a> {
    /// No animated bone channel: the static path (most doodads: trees, fences, rocks).
    Static,
    /// Only global-sequence channels (a candelabra's glow): a [`GlobalSeqDrive`], no player.
    GlobalSeqOnly,
    /// The loader-idle seed moves bones (flags, windmills, torch flames): an `AnimationPlayer`
    /// looping this clip. The seed is animation id 0 resolved through the model's
    /// `playableAnimationLookup`, not the file-order first sequence the name suggests.
    FirstSeq(&'a AnimClip),
}

/// The animation-event tags that make a placed doodad sound: `$DSL` (a looping emitter), `$DSO` (a
/// positioned one-shot), `$DSE` (the stop that releases a `$DSL`) and `$SND`, the markers the
/// reference's doodad event callback `0x6951e0` routes on.
pub(crate) const SOUND_EVENT_TAGS: [&[u8; 4]; 4] = [b"$DSL", b"$DSO", b"$DSE", b"$SND"];

/// Whether the model's arm chain (the loader-idle seed and its variations, the only sequences a
/// placed doodad plays) carries a sound-event marker. Kept out of [`classify`], which answers what
/// the model renders: the reference arms every placed doodad (`0x695100` → `0x7121a0`), and its
/// event track runs on that clock even when no bone moves (`KalidarStreetLamp01.m2`'s `$DSL`).
pub(crate) fn arms_for_sound(anims: &ModelAnimations) -> bool {
    let Some(idle) = anims.idle_clip() else {
        return false;
    };
    anims
        .clips
        .iter()
        .filter(|c| c.anim_id == idle.anim_id)
        .any(|c| {
            c.events
                .iter()
                .any(|e| SOUND_EVENT_TAGS.contains(&&e.ident))
        })
}

/// Classify a placed model; a boneless one is `Static`, with no joints for a skin to index.
pub fn classify<'a>(
    skeleton: &ModelSkeleton,
    animations: Option<&'a ModelAnimations>,
) -> DoodadAnimTier<'a> {
    let Some(anims) = animations else {
        return DoodadAnimTier::Static;
    };
    if skeleton.joints.is_empty() {
        return DoodadAnimTier::Static;
    }
    // `first_seq` is the content gate, not the arm's identity (`idle_seq`): a seed that holds bind
    // pose renders identically to the static mesh.
    match anims.first_seq.and_then(|i| anims.clips.get(i)) {
        Some(clip) => DoodadAnimTier::FirstSeq(clip),
        None if !anims.global_bones.is_empty() => DoodadAnimTier::GlobalSeqOnly,
        None => DoodadAnimTier::Static,
    }
}

/// What [`spawn_anim_host`] set up for one placement; the [`RigPose`] rides here by value until
/// [`Self::finish`], so anchors ([`Self::anchor`]) are minted only for bones something rides.
pub struct AnimHostSpawn {
    pub root: Entity,
    /// The pose buffer, inserted on `root` by [`Self::finish`] once every anchor is minted.
    pose: RigPose,
    pub inverse_bindposes: Handle<SkinnedMeshInverseBindposes>,
    /// The armed clip's node and duration, `None` on the gseq-only tier.
    pub(crate) clip: Option<(AnimationNodeIndex, f32)>,
    /// The file sequence slot the load arm seeded ([`AnimClip::seq_index`]); the first re-roll
    /// replaces it, as the holder's `0x695100` arm does, so the current slot is read off the host's
    /// player ([`crate::particles::EmitClock::Host`]). `None` on the gseq-only tier.
    pub(crate) seq: Option<usize>,
    /// The animation id the arm rolls variations over; `None` on the gseq-only tier.
    pub(crate) anim_id: Option<u16>,
}

impl AnimHostSpawn {
    /// The anchor entity for `bone`, spawned on first demand and shared after
    /// ([`RigPose::anchor_for`]); `None` for a bone outside the skeleton.
    pub fn anchor(&mut self, commands: &mut Commands, bone: u16) -> Option<Entity> {
        self.pose.anchor_for(commands, self.root, bone)
    }

    /// The skeleton's bone count, the palette allocation size.
    pub fn bones(&self) -> u32 {
        self.pose.locals.len() as u32
    }

    /// Attach the pose buffer once the anchors are minted; returns the `(bone, anchor)` registry.
    pub fn finish(self, commands: &mut Commands) -> Vec<(u16, Entity)> {
        let anchors = self.pose.anchors.clone();
        commands.entity(self.root).insert(self.pose);
        anchors
    }
}

/// Whether [`spawn_anim_host`] would rig this model, by the same tier and capture gates, so skinned
/// twins are built only for placed models that draw them.
pub fn wants_rig(m: &M2Model) -> bool {
    !crate::dev_state::deterministic_run()
        && !matches!(
            classify(&m.skeleton, m.animations.as_ref()),
            DoodadAnimTier::Static
        )
}

/// Spawn the animation host for one placed M2 that animates: a root at the placement transform
/// with the collapsed [`RigPose`] and, per tier, an `AnimationPlayer` looping the loader-idle seed
/// (the reference's load arm `0x70ebd0` → `0x7121a0(bone 0, animation id 0, linkFlag=1)`) and/or
/// a [`GlobalSeqDrive`]. `None` keeps the static path.
pub fn spawn_anim_host(
    commands: &mut Commands,
    m: &M2Model,
    transform: Transform,
) -> Option<AnimHostSpawn> {
    // Captures keep every doodad static, so their frames are deterministic.
    if crate::dev_state::deterministic_run() {
        return None;
    }
    let tier = classify(&m.skeleton, m.animations.as_ref());
    // The sound arm, orthogonal to the tier: a bind-posed model still runs its event track, as in
    // the reference, so it gets a host with the arm bookkeeping but no `AnimationPlayer`.
    let sound_arm = m.animations.as_ref().is_some_and(arms_for_sound);
    if matches!(tier, DoodadAnimTier::Static) && !sound_arm {
        return None;
    }
    let anims = m
        .animations
        .as_ref()
        .expect("animated tier or sound arm ⇒ animations");
    let root = commands
        .spawn((transform, Visibility::default()))
        .vis_chain_only()
        .id();
    // The caller allocates the palette slot: lazily on the terrain-stream lane (`LazyRig`), eagerly
    // for a gate-less caller such as quest markers, whose host would otherwise never skin.
    let pose = RigPose::new(root, &m.skeleton);
    let mut clip_info = None;
    let mut armed_seq = None;
    let mut arm_id = None;
    if let DoodadAnimTier::FirstSeq(head) = tier {
        // The loader's var-0 seed only (`0x70ebd0`). The effective arm is the holder's
        // `variationIdx = -1` op4 (`0x695100`), here `reroll_doodad_variation`'s first pass, the
        // same frame, since the host is born with an expired window.
        let mut player = AnimationPlayer::default();
        player.play(head.node).repeat();
        commands.entity(root).insert((
            player,
            AnimationGraphHandle(anims.graph.clone()),
            // For the re-roll and the per-sequence consumers resolving off this host.
            anims.clone(),
        ));
        clip_info = Some((head.node, head.duration));
        armed_seq = Some(head.seq_index);
        arm_id = Some(head.anim_id);
    } else if sound_arm {
        // The clock-only arm: the re-roll's query needs `ModelAnimations`; nothing here poses.
        let head = anims.idle_clip().expect("sound arm ⇒ an idle clip");
        clip_info = Some((head.node, head.duration));
        arm_id = Some(head.anim_id);
        commands.entity(root).insert(anims.clone());
        // `armed_seq` stays `None`: it alone moves the placement's emitters onto `EmitClock::Host`,
        // which reads a player this host lacks. The sound clock reads `clip` and `armed_at`.
    }
    if let Some(drive) = GlobalSeqDrive::new_rig(&anims.global_bones, m.skeleton.joints.len()) {
        commands.entity(root).insert(drive);
    }
    Some(AnimHostSpawn {
        root,
        pose,
        inverse_bindposes: m.inverse_bindposes.clone(),
        clip: clip_info,
        seq: armed_seq,
        anim_id: arm_id,
    })
}

/// The anim root of one animated placement: the draw gate's state and what to re-arm on resume.
/// It despawns with its tile, its bone anchors with it.
#[derive(Component)]
pub struct DoodadAnimHost {
    /// The placement's skinned submeshes; animation runs while any of them is drawn.
    pub(crate) meshes: Vec<Entity>,
    /// The draw-set gate of a meshless (particles-only) host: the [`crate::particles::EmitterFade`]
    /// its emitters, ribbons and lights were built with, so they freeze together. It carries the
    /// prop's building (instance and room), without which a sealed room would park its own props.
    pub(crate) fade: crate::particles::EmitterFade,
    /// The armed clip's graph node and duration (secs); `None` on the gseq-only tier.
    pub(crate) clip: Option<(AnimationNodeIndex, f32)>,
    /// `Time::elapsed_secs` at the current arm, the player's clock origin: a resume seeks to
    /// `(now − armed_at) mod duration`. The [`GlobalSeqDrive`] keeps its own attach anchor instead.
    pub(crate) armed_at: f32,
    /// The end of the armed play-window on the shared clock, the reference's `windowHi`
    /// (`[model+0xac]`); `now ≥ window_hi` re-rolls. Born at `NEG_INFINITY`, so frame one arms.
    pub(crate) window_hi: f32,
    /// The animation id to re-roll over; `None` on the gseq-only tier, which has no window.
    pub(crate) anim_id: Option<u16>,
    /// Whether the host was ticking last frame (edge-triggered pause/resume).
    pub active: bool,
    /// `Time::elapsed_secs` when `active` last went false: the reaper demotes the longest-parked.
    pub(crate) parked_at: f32,
}

impl DoodadAnimHost {
    /// The armed clip's `(node, seek)` on the shared clock, `(now − armed_at) mod duration`. It
    /// answers while parked, but the reference fires no events for a doodad culled out of its
    /// scene list (`[CM2Scene+0x20]`), so a consumer reads [`DoodadAnimHost::active`] beside it.
    pub fn arm_clock(&self, now: f32) -> Option<(AnimationNodeIndex, f32)> {
        let (node, duration) = self.clip?;
        // A zero-length clip has no phase to compute; it sits at its own t = 0.
        let seek = if duration > 0.0 {
            (now - self.armed_at).rem_euclid(duration)
        } else {
            0.0
        };
        Some((node, seek))
    }
}

/// The doodad's self-sustaining re-arm: when the armed play-window ends, roll a fresh
/// frequency-weighted variation of the same animation id, snap to it (`blendFlag = 0` at
/// `0x6951c8`, no cross-fade) and write the next window. An undrawn host only updates
/// [`DoodadAnimHost::clip`], for [`gate_doodad_anim`]'s resume to arm.
///
/// Deviation: this runs for every resident host, where the reference stops a doodad culled out
/// of its scene list (`[CM2Scene+0x20]`) and re-arms it at once on re-link (`0x7074b0`,
/// `0x683f80`), because gating it so would re-roll the whole field on the frame the camera turns
/// back.
fn reroll_doodad_variation(
    time: Res<Time>,
    mut rng: ResMut<benilla_assets::AnimRng>,
    mut hosts: Query<(
        &mut DoodadAnimHost,
        &ModelAnimations,
        Option<&mut AnimationPlayer>,
    )>,
) {
    let now = time.elapsed_secs();
    for (mut host, anims, player) in &mut hosts {
        let Some(anim_id) = host.anim_id else {
            continue; // gseq-only: never armed
        };
        if now < host.window_hi {
            continue;
        }
        let Some(clip) = anims.pick_variation(anim_id, rng.draw()) else {
            // No chain for this id: stop asking.
            host.anim_id = None;
            continue;
        };
        let (node, duration) = (clip.node, clip.duration);
        let replay = rng.replay_count(clip.replay);
        host.armed_at = now;
        // `span · R`, one loop for `replay = (0, 0)`; a zero-duration clip gets a minimum window.
        host.window_hi = now + (duration * replay as f32).max(f32::EPSILON);
        host.clip = Some((node, duration));
        if host.active {
            if let Some(mut p) = player {
                p.stop_all(); // the snap
                p.play(node).repeat();
            }
        }
    }
}

/// The draw gate: stop a doodad's animation while it is not drawn, and on resume seek the player
/// to the arm's shared-clock phase. Runs before [`AnimationSystems`], so the seek lands the same
/// frame.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn gate_doodad_anim(
    time: Res<Time>,
    mut hosts: Query<(
        Entity,
        &mut DoodadAnimHost,
        Option<&lazy::LazyRig>,
        Option<&RigPose>,
        Has<crate::rig_palette::RigSkin>,
        Option<&mut AnimationPlayer>,
        Option<&mut GlobalSeqDrive>,
    )>,
    vis: Query<&Visibility>,
    cam: Query<
        (
            Ref<GlobalTransform>,
            &bevy::camera::primitives::Frustum,
            Ref<bevy::camera::Projection>,
            // The seat's own write: `GlobalTransform` propagates after this system, so on a
            // teleport frame only the local shows the camera moved.
            Option<Ref<Transform>>,
        ),
        With<crate::view::WorldCamera>,
    >,
    // With no part's `Visibility` changed and the camera still, every verdict is last frame's.
    changed_vis: Query<(), (Changed<Visibility>, With<crate::model_render::ModelPart>)>,
    // The draw-set inputs (far clip, exterior-window gate, camera room, portal instances), the
    // same `SystemParam` the particle and ribbon sims read, so a placement's riders agree.
    scene: crate::particles::sim::SceneGates,
    // The lazy-rig lane's wake half: a drawn host without a slot promotes here.
    mut palettes: ResMut<crate::rig_palette::RigPalettes>,
    ibps: Res<Assets<SkinnedMeshInverseBindposes>>,
    worlds: Query<&GlobalTransform>,
    mut twin_parts: lazy::TwinParts,
    mut commands: Commands,
    mut logged: Local<bool>,
) {
    // One log line per session, the first frame any host exists.
    if !*logged && !hosts.is_empty() {
        *logged = true;
        info!("doodad anim: first host armed");
    }
    let now = time.elapsed_secs();
    let world_cam = cam.single().ok();
    let (farclip, exterior_gate, camera_instance) =
        scene.scene(world_cam.as_ref().map(|(tf, _, proj, _)| (&**tf, &**proj)));
    let verdicts_still = world_cam.as_ref().is_some_and(|(tf, _, proj, local)| {
        !tf.is_changed() && !proj.is_changed() && !local.as_ref().is_some_and(|l| l.is_changed())
    }) && !scene.changed()
        && changed_vis.is_empty();
    for (entity, mut host, lazy, pose, has_rig, player, drive) in &mut hosts {
        // A host born this frame has no verdict to reuse (`active` starts false).
        let drawn = if verdicts_still && !host.is_added() {
            host.active
        } else if host.meshes.is_empty() {
            // Meshless: the reference ticks any model in the draw set, and a 0-batch model is
            // admitted on its fade sphere like its emitters, through the same `in_draw_set`.
            let fade = &host.fade;
            world_cam.as_ref().is_some_and(|(cam_tf, frustum, _, _)| {
                fade.in_draw_set(
                    cam_tf.translation(),
                    Vec3::from(cam_tf.forward()),
                    farclip,
                    frustum.intersects_sphere(
                        &bevy::camera::primitives::Sphere {
                            center: fade.center.into(),
                            radius: fade.radius,
                        },
                        false,
                    ),
                    fade.exterior_admitted(&exterior_gate, camera_instance),
                    scene.room_admits(fade),
                )
            })
        } else {
            host.meshes
                .iter()
                .any(|&e| vis.get(e).is_ok_and(|v| *v != Visibility::Hidden))
        };
        // The lazy-rig promote, retried every frame the host stays drawn so a full table is only
        // a delay. It needs a second drawn frame (`host.active`): on its first, a spawned part's
        // `Visibility` is still the `Inherited` default the fade and cull authorities have not
        // judged, so every far doodad of a new tile reads drawn.
        if drawn && host.active && !has_rig {
            if let Some(lazy) = lazy {
                lazy::promote_lazy_rig(
                    &mut commands,
                    &mut palettes,
                    &ibps,
                    &worlds,
                    entity,
                    lazy,
                    pose,
                    &mut twin_parts,
                );
            }
        }
        if drawn == host.active {
            continue;
        }
        host.active = drawn;
        // `AnimParked` holds the pose evaluation, compose and finalize; these commands apply
        // before `AnimationSystems`, so a resume's re-arm evaluates the frame the marker drops.
        if drawn {
            commands.entity(entity).remove::<AnimParked>();
        } else {
            host.parked_at = now; // the reaper's age key
            commands.entity(entity).insert(AnimParked);
        }
        if let Some(mut p) = player {
            if drawn {
                if let Some((node, duration)) = host.clip {
                    let anim = p.start(node);
                    anim.repeat();
                    if duration > 0.0 {
                        // Phase from the current arm, not from spawn: the variation re-arms.
                        anim.seek_to((now - host.armed_at).rem_euclid(duration));
                    }
                }
            } else {
                // Stop, not pause: `animate_targets` still applies a paused animation each frame.
                p.stop_all();
            }
        }
        if let Some(mut d) = drive {
            // No re-seek: the drive's cursor, sceneNow − attach, is a pure function of the clock.
            d.set_paused(!drawn);
        }
    }
}

pub struct DoodadAnimPlugin;

impl Plugin for DoodadAnimPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UvAnimMaterials>();
        app.init_resource::<TintAnimMaterials>();
        // The one `rand()` stream: the reference seeds it once per process before `WinMain`
        // (`srand(GetTickCount())`, `0x5d1c70`); a capture leaves it unseeded.
        app.init_resource::<benilla_assets::AnimRng>();
        let deterministic = crate::dev_state::deterministic_run();
        app.world_mut()
            .resource_mut::<benilla_assets::AnimRng>()
            .seed_for_session(deterministic);
        // The re-roll runs before the draw gate, or a host re-appearing as its window expires
        // resumes the old variation for a frame.
        app.add_systems(
            PostUpdate,
            (reroll_doodad_variation, gate_doodad_anim)
                .chain()
                .before(AnimationSystems),
        );
        app.add_systems(Update, lazy::reap_parked_rigs);
        // `sample_mat_anim` runs before the visibility authority, which composes its value the
        // same frame; the material tick runs after, since its draw gate reads that verdict.
        app.add_systems(
            Update,
            (
                sample_mat_anim.before(crate::model_render::ModelVisSet),
                tick_anim_materials.after(crate::model_render::ModelVisSet),
                // After the tick, so the probe's `row` column is this frame's write.
                mat_anim::matanim_probe.after(tick_anim_materials),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_assets::{GlobalBone, GlobalSeqChannel};

    fn clip(anim_id: u16) -> AnimClip {
        AnimClip {
            anim_id,
            seq_index: 0,
            node: AnimationNodeIndex::new(1),
            looping: true,
            duration: 2.0,
            move_speed: 0.0,
            blend_time: 0.0,
            bounds_center: Vec3::ZERO,
            bounds_radius: 0.0,
            bounds_min: Vec3::ZERO,
            bounds_max: Vec3::ZERO,
            events: Vec::new().into(),
            arm_nodes: None,
            upper_node: None,
            frequency: 0,
            replay: (0, 0),
            poses_bones: true,
        }
    }

    fn anims(clips: Vec<AnimClip>, first_seq: Option<usize>, gseq: bool) -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips,
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            hand_close: [None, None],
            global_bones: if gseq {
                vec![GlobalBone {
                    bone: 1,
                    translation: None,
                    rotation: None,
                    scale: Some(GlobalSeqChannel {
                        period: 1.167,
                        keys: vec![(0.0, Vec3::ONE), (0.5, Vec3::splat(1.2))],
                    }),
                }]
            } else {
                Vec::new()
            },
            first_seq,
            pose: Default::default(),
        }
    }

    /// One batch's per-sequence alpha, as the bake emits it: slot 0 hidden, slot 1 visible.
    fn two_seq_alpha() -> std::sync::Arc<benilla_formats::AlphaAnim> {
        let hidden = benilla_formats::ScalarAnim {
            period: 0.0,
            step: true,
            wrap: true, // period 0: a constant has no clock
            gseq: false,
            keys: vec![(0.0, 0.0)],
        };
        std::sync::Arc::new(
            benilla_formats::AlphaAnim::new(vec![
                benilla_formats::AlphaSeq {
                    color: None,
                    weight: Some(hidden),
                },
                benilla_formats::AlphaSeq::default(),
            ])
            .expect("a hiding sequence is worth carrying"),
        )
    }

    fn seq_clip(anim_id: u16, seq_index: usize, node: usize) -> AnimClip {
        AnimClip {
            seq_index,
            node: AnimationNodeIndex::new(node),
            ..clip(anim_id)
        }
    }

    #[test]
    fn hosted_mat_anim_follows_the_host_sequence() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, sample_mat_anim);

        // Two clips: graph node 1 is file sequence 0 (which hides the batch), node 2 is slot 1.
        let anims = anims(vec![seq_clip(0, 0, 1), seq_clip(1, 1, 2)], Some(0), false);
        let mut player = AnimationPlayer::default();
        player.play(AnimationNodeIndex::new(1)).repeat();
        let host = app.world_mut().spawn((player, anims)).id();
        let part = app
            .world_mut()
            .spawn(MatAnim::following(two_seq_alpha(), host))
            .id();

        app.update();
        assert_eq!(
            app.world().entity(part).get::<MatAnim>().unwrap().current,
            0.0,
            "playing sequence 0 ⇒ the batch is culled"
        );

        let world = app.world_mut();
        let mut entity = world.entity_mut(host);
        {
            let mut p = entity.get_mut::<AnimationPlayer>().unwrap();
            p.stop_all();
            p.play(AnimationNodeIndex::new(2)).repeat();
        }
        app.update();
        assert_eq!(
            app.world().entity(part).get::<MatAnim>().unwrap().current,
            1.0,
            "playing sequence 1 ⇒ the batch draws"
        );
    }

    /// With no player yet, slot 0 at t = 0, so the batch does not flash visible for a frame.
    #[test]
    fn hosted_mat_anim_without_a_player_reads_the_first_sequence() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, sample_mat_anim);
        let host = app.world_mut().spawn(Transform::default()).id();
        let part = app
            .world_mut()
            .spawn(MatAnim::following(two_seq_alpha(), host))
            .id();
        app.update();
        assert_eq!(
            app.world().entity(part).get::<MatAnim>().unwrap().current,
            0.0
        );
    }

    #[test]
    fn pinned_mat_anim_reads_its_own_slot() {
        let a = two_seq_alpha();
        let doodad = MatAnim::new(a.clone(), 0.0, false);
        assert_eq!(doodad.current, 0.0, "slot 0 — the one-time load arm");
        assert!(!doodad.composes_unit_tag());
        let effect = MatAnim::driving_tag(a, 0.0, Some(1));
        assert_eq!(effect.current, 1.0, "the sequence the fx rig armed");
        assert!(effect.drives_tag);
        assert!(!effect.composes_unit_tag());
    }

    fn skeleton(joints: usize) -> ModelSkeleton {
        ModelSkeleton {
            joints: (0..joints)
                .map(|_| benilla_assets::ModelJoint {
                    parent: -1,
                    local_translation: Vec3::ZERO,
                    billboard: None,
                    parent_arm: None,
                })
                .collect(),
            spine_bone: None,
            head_bone: None,
        }
    }

    #[test]
    fn classify_picks_the_measured_tiers() {
        // No ModelAnimations at all (a barrel): static.
        assert!(matches!(
            classify(&skeleton(3), None),
            DoodadAnimTier::Static
        ));
        // Boneless model: static even with parsed animations.
        let a = anims(vec![clip(0)], Some(0), true);
        assert!(matches!(
            classify(&skeleton(0), Some(&a)),
            DoodadAnimTier::Static
        ));
        // A motionless first sequence, no gseq (a posed tree): static.
        let a = anims(vec![clip(0)], None, false);
        assert!(matches!(
            classify(&skeleton(3), Some(&a)),
            DoodadAnimTier::Static
        ));
        // Gseq only (a candelabra glow pulse): the player-less tier.
        let a = anims(Vec::new(), None, true);
        assert!(matches!(
            classify(&skeleton(3), Some(&a)),
            DoodadAnimTier::GlobalSeqOnly
        ));
        // A moving first sequence (a flag / the torch flame bone): loop its clip.
        let a = anims(vec![clip(0)], Some(0), true);
        assert!(matches!(
            classify(&skeleton(3), Some(&a)),
            DoodadAnimTier::FirstSeq(c) if c.anim_id == 0
        ));
        // A clock-only model (clips but no `first_seq`: its animation lives in emitter tracks)
        // stays Static; the entity lane arms its clock off `idle_clip`.
        let a = anims(vec![clip(0)], None, false);
        assert!(matches!(
            classify(&skeleton(3), Some(&a)),
            DoodadAnimTier::Static
        ));
        // ...and one that also pulses on a global sequence takes the player-less tier, not a rig.
        let a = anims(vec![clip(0)], None, true);
        assert!(matches!(
            classify(&skeleton(3), Some(&a)),
            DoodadAnimTier::GlobalSeqOnly
        ));
    }

    /// `BlastedLandsLightningbolt01.M2`'s chain: two variations of animation id 0, weights 31129
    /// and 1638, so slot 1, where its emitters key the whole burst, carries 5.0 %.
    fn lightning_anims() -> ModelAnimations {
        let mut a = anims(vec![seq_clip(0, 0, 1), seq_clip(0, 1, 2)], Some(0), false);
        a.clips[0].frequency = 31129;
        a.clips[0].duration = 1.333;
        a.clips[1].frequency = 1638;
        a.clips[1].duration = 1.300;
        a
    }

    fn reroll_app() -> App {
        // No `TimePlugin`: its real clock would clobber the manual advances that step windows.
        let mut app = App::new();
        app.init_resource::<Time>();
        app.init_resource::<benilla_assets::AnimRng>();
        app.add_systems(Update, reroll_doodad_variation);
        app
    }

    fn lightning_host(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((
                DoodadAnimHost {
                    meshes: Vec::new(),
                    fade: crate::particles::EmitterFade::sphere(1.0, Vec3::ZERO),
                    clip: None,
                    armed_at: 0.0,
                    window_hi: f32::NEG_INFINITY,
                    anim_id: Some(0),
                    active: true,
                    parked_at: 0.0,
                },
                lightning_anims(),
                AnimationPlayer::default(),
            ))
            .id()
    }

    #[test]
    fn a_placed_doodad_rerolls_its_variation_every_play_window() {
        let mut app = reroll_app();
        let host = lightning_host(&mut app);

        // Frame 1: born with an expired window, the holder's `variationIdx = -1` arm lands at once.
        app.update();
        let armed = |app: &App| {
            app.world()
                .entity(host)
                .get::<DoodadAnimHost>()
                .unwrap()
                .clip
        };
        assert!(armed(&app).is_some(), "the first frame arms");

        let mut seen = std::collections::HashMap::new();
        for _ in 0..400 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(1400));
            app.update();
            let h = app.world().entity(host).get::<DoodadAnimHost>().unwrap();
            let (node, duration) = h.clip.expect("a window always leaves something armed");
            // R = 1 for `replay = (0, 0)`, so the window is exactly the armed clip's own length.
            assert!(
                (h.window_hi - h.armed_at - duration).abs() < 1e-3,
                "window {} != clip duration {duration}",
                h.window_hi - h.armed_at
            );
            *seen.entry(node).or_insert(0u32) += 1;
        }

        let slot0 = seen.get(&AnimationNodeIndex::new(1)).copied().unwrap_or(0);
        let slot1 = seen.get(&AnimationNodeIndex::new(2)).copied().unwrap_or(0);
        assert_eq!(slot0 + slot1, 400, "every window arms one of the two");
        assert!(
            slot1 > 0,
            "the 5 % strike variation never came up in 400 windows — the bolt is stuck again"
        );
        assert!(slot0 > 0, "the 95 % silent variation never came up");
        // A sanity band, not a distribution test: this seed lands on 10 (~2.3σ low). Over 200 000
        // draws at two per window the rate is 4.9665 % against the authored 4.9988 %.
        assert!(
            (2..=60).contains(&slot1),
            "slot 1 came up {slot1}/400 — far enough off its authored 5 % to suspect the roll"
        );
    }

    /// Global sequences loop with no arming, so the gseq-only tier opens no window.
    #[test]
    fn a_gseq_only_host_never_rerolls() {
        let mut app = reroll_app();
        let host = app
            .world_mut()
            .spawn((
                DoodadAnimHost {
                    meshes: Vec::new(),
                    fade: crate::particles::EmitterFade::sphere(1.0, Vec3::ZERO),
                    clip: None,
                    armed_at: 0.0,
                    window_hi: f32::NEG_INFINITY,
                    anim_id: None,
                    active: true,
                    parked_at: 0.0,
                },
                anims(Vec::new(), None, true),
                AnimationPlayer::default(),
            ))
            .id();
        for _ in 0..8 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(1400));
            app.update();
        }
        let h = app.world().entity(host).get::<DoodadAnimHost>().unwrap();
        assert!(h.clip.is_none(), "nothing was ever armed");
        assert_eq!(h.window_hi, f32::NEG_INFINITY, "no window was ever opened");
    }

    /// The deviation on [`reroll_doodad_variation`]: an undrawn host cycles, its player untouched.
    #[test]
    fn an_undrawn_host_keeps_cycling() {
        let mut app = reroll_app();
        let host = lightning_host(&mut app);
        app.world_mut()
            .entity_mut(host)
            .get_mut::<DoodadAnimHost>()
            .unwrap()
            .active = false;

        app.update();
        let first = app
            .world()
            .entity(host)
            .get::<DoodadAnimHost>()
            .unwrap()
            .window_hi;
        for _ in 0..3 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(1400));
            app.update();
        }
        let h = app.world().entity(host).get::<DoodadAnimHost>().unwrap();
        assert!(
            h.window_hi > first,
            "an undrawn host still opened new windows"
        );
        assert!(
            h.clip.is_some(),
            "and still has something for the resume to arm"
        );
        assert_eq!(
            app.world()
                .entity(host)
                .get::<AnimationPlayer>()
                .unwrap()
                .playing_animations()
                .count(),
            0,
            "but its stopped player was left alone"
        );
    }

    #[test]
    fn gate_pauses_hidden_and_resumes_on_the_shared_clock() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        // The gate's params must resolve, though this meshed host never reads these.
        app.init_resource::<crate::view::ViewDistance>();
        app.init_resource::<crate::wmo_portal::ExteriorWindows>();
        app.init_resource::<crate::wmo_portal::CameraInteriorClaim>();
        app.init_resource::<crate::rig_palette::RigPalettes>();
        app.init_asset::<SkinnedMeshInverseBindposes>();
        app.add_systems(Update, gate_doodad_anim);

        let mesh = app.world_mut().spawn(Visibility::Inherited).id();
        let node = AnimationNodeIndex::new(1);
        let mut player = AnimationPlayer::default();
        player.play(node).repeat();
        let joint = app.world_mut().spawn(Transform::default()).id();
        let drive = GlobalSeqDrive::new(
            &anims(Vec::new(), None, true).global_bones,
            &[Entity::PLACEHOLDER, joint],
        )
        .expect("one gseq bone maps");
        let host = app
            .world_mut()
            .spawn((
                DoodadAnimHost {
                    meshes: vec![mesh],
                    fade: crate::particles::EmitterFade::sphere(1.0, Vec3::ZERO),
                    clip: Some((node, 2.0)),
                    armed_at: 0.0,
                    // Far future: no window may expire and change the armed clip mid-test.
                    window_hi: f32::INFINITY,
                    anim_id: Some(0),
                    active: true,
                    parked_at: 0.0,
                },
                player,
                drive,
            ))
            .id();

        app.update();
        let playing = |app: &mut App, e: Entity| {
            app.world_mut()
                .entity(e)
                .get::<AnimationPlayer>()
                .unwrap()
                .playing_animations()
                .count()
        };
        assert_eq!(playing(&mut app, host), 1, "drawn ⇒ playing");

        *app.world_mut()
            .entity_mut(mesh)
            .get_mut::<Visibility>()
            .unwrap() = Visibility::Hidden;
        app.update();
        assert_eq!(playing(&mut app, host), 0, "hidden ⇒ stopped");
        assert!(
            !app.world()
                .entity(host)
                .get::<DoodadAnimHost>()
                .unwrap()
                .active
        );

        *app.world_mut()
            .entity_mut(mesh)
            .get_mut::<Visibility>()
            .unwrap() = Visibility::Inherited;
        app.update();
        assert_eq!(playing(&mut app, host), 1, "re-drawn ⇒ re-armed");
        let world = app.world();
        let p = world.entity(host).get::<AnimationPlayer>().unwrap();
        let active = p.animation(node).expect("the first-seq node is active");
        assert!(
            active.seek_time() >= 0.0 && active.seek_time() < 2.0,
            "seek lands inside the loop: {}",
            active.seek_time()
        );
    }

    /// The control is a map doodad at the same spot with no building, which the sealed room stops.
    #[test]
    fn a_sealed_room_keeps_its_own_meshless_props_animating() {
        use crate::wmo_portal::{
            CameraInteriorClaim, ExteriorWindows, InteriorClaim, WmoGroupVis, WmoPortalInstance,
            WmoRoom,
        };
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_resource::<crate::view::ViewDistance>();
        app.init_resource::<crate::rig_palette::RigPalettes>();
        app.init_asset::<SkinnedMeshInverseBindposes>();
        app.add_systems(Update, gate_doodad_anim);

        // The camera stands inside a sealed room of `building`, with no exterior window.
        let building = app
            .world_mut()
            .spawn(WmoPortalInstance {
                handle: Handle::default(),
                world_from_local: bevy::math::Affine3A::IDENTITY,
                name_set: 0,
                visible: vec![true],
                interior_fog: vec![false],
                liquid_visited: vec![false],
                flooded: vec![None],
            })
            .id();
        app.insert_resource(ExteriorWindows::Windows(Vec::new()));
        app.insert_resource(CameraInteriorClaim(Some(InteriorClaim {
            room: WmoRoom {
                instance: building,
                group: 0,
            },
            exterior_visible: false,
        })));

        // Looking straight at both hosts, 20 yd out and well inside the wall.
        let seat =
            Transform::from_xyz(0.0, 0.0, 0.0).looking_at(Vec3::new(0.0, 0.0, -20.0), Vec3::Y);
        let projection = bevy::camera::Projection::from(bevy::camera::PerspectiveProjection {
            far: 5000.0,
            ..default()
        });
        let frustum = bevy::camera::primitives::Frustum::from_clip_from_world(
            &(projection.get_clip_from_view() * GlobalTransform::from(seat).affine().inverse()),
        );
        app.world_mut().spawn((
            crate::view::WorldCamera,
            GlobalTransform::from(seat),
            seat,
            frustum,
            projection,
        ));

        let node = AnimationNodeIndex::new(1);
        let mut meshless = |fade: crate::particles::EmitterFade| {
            app.world_mut()
                .spawn(DoodadAnimHost {
                    meshes: Vec::new(), // 0 render batches
                    fade,
                    clip: Some((node, 2.0)),
                    armed_at: 0.0,
                    window_hi: f32::INFINITY,
                    anim_id: Some(0),
                    active: false,
                    parked_at: 0.0,
                })
                .id()
        };
        let center = Vec3::new(0.0, 0.0, -20.0);
        let prop = meshless(crate::particles::EmitterFade {
            instance: Some(building),
            room: Some(WmoGroupVis {
                instance: building,
                groups: [0u16].as_slice().into(),
            }),
            ..crate::particles::EmitterFade::sphere(2.0, center)
        });
        let map_doodad = meshless(crate::particles::EmitterFade::sphere(2.0, center));

        app.update();
        let active = |app: &App, e: Entity| {
            app.world()
                .entity(e)
                .get::<DoodadAnimHost>()
                .unwrap()
                .active
        };
        assert!(
            active(&app, prop),
            "a prop of the camera's OWN building is not exterior to it — it keeps animating, and \
             its $DSL keeps firing"
        );
        assert!(
            !active(&app, map_doodad),
            "the control: a map doodad carries no building, so the sealed room stops it"
        );
    }

    /// The gate runs before transform propagation, so on a teleport frame only the camera's local
    /// has moved, and the verdict reuse must still re-judge every host.
    #[test]
    fn a_reseated_camera_is_not_a_still_scene() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_resource::<crate::view::ViewDistance>();
        app.init_resource::<crate::wmo_portal::ExteriorWindows>();
        app.init_resource::<crate::wmo_portal::CameraInteriorClaim>();
        app.init_resource::<crate::rig_palette::RigPalettes>();
        app.init_asset::<SkinnedMeshInverseBindposes>();
        app.add_systems(Update, gate_doodad_anim);

        // A camera 1000 yd off, looking away: not drawn. With no propagation, `GlobalTransform`
        // never follows `Transform`, as on the real snap frame.
        let far =
            Transform::from_xyz(1000.0, 0.0, 0.0).looking_at(Vec3::new(2000.0, 0.0, 0.0), Vec3::Y);
        let projection = bevy::camera::Projection::from(bevy::camera::PerspectiveProjection {
            far: 5000.0,
            ..default()
        });
        let frustum = bevy::camera::primitives::Frustum::from_clip_from_world(
            &(projection.get_clip_from_view() * GlobalTransform::from(far).affine().inverse()),
        );
        let cam = app
            .world_mut()
            .spawn((
                crate::view::WorldCamera,
                GlobalTransform::from(far),
                far,
                frustum,
                projection,
            ))
            .id();
        let node = AnimationNodeIndex::new(1);
        let mut player = AnimationPlayer::default();
        player.play(node).repeat();
        let joint = app.world_mut().spawn(Transform::default()).id();
        let drive = GlobalSeqDrive::new(
            &anims(Vec::new(), None, true).global_bones,
            &[Entity::PLACEHOLDER, joint],
        )
        .expect("one gseq bone maps");
        let host = app
            .world_mut()
            .spawn((
                DoodadAnimHost {
                    meshes: Vec::new(),
                    fade: crate::particles::EmitterFade::sphere(1.0, Vec3::ZERO),
                    clip: Some((node, 2.0)),
                    armed_at: 0.0,
                    window_hi: f32::INFINITY,
                    anim_id: Some(0),
                    active: true,
                    parked_at: 0.0,
                },
                player,
                drive,
            ))
            .id();
        let active = |app: &App| {
            app.world()
                .entity(host)
                .get::<DoodadAnimHost>()
                .unwrap()
                .active
        };
        app.update();
        assert!(!active(&app), "far and facing away ⇒ parked");
        app.update();
        app.update();
        assert!(!active(&app), "still ⇒ the verdict is reused, still parked");

        // The snap: the seat writes the local next to the host, facing it; the global stays stale.
        let near = Transform::from_xyz(0.0, 0.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y);
        let near_frustum = bevy::camera::primitives::Frustum::from_clip_from_world(
            &(bevy::camera::Projection::from(bevy::camera::PerspectiveProjection {
                far: 5000.0,
                ..default()
            })
            .get_clip_from_view()
                * GlobalTransform::from(near).affine().inverse()),
        );
        {
            let mut e = app.world_mut().entity_mut(cam);
            *e.get_mut::<Transform>().unwrap() = near;
            *e.get_mut::<bevy::camera::primitives::Frustum>().unwrap() = near_frustum;
            // Written without a change tick, so the local is this frame's only moved signal.
            *e.get_mut::<GlobalTransform>()
                .unwrap()
                .bypass_change_detection() = GlobalTransform::from(near);
        }
        app.update();
        assert!(active(&app), "the snap frame re-judges the host: drawn");
    }

    #[test]
    fn arm_clock_wraps_on_the_shared_clock() {
        let mut host = DoodadAnimHost {
            meshes: Vec::new(),
            fade: crate::particles::EmitterFade::sphere(0.0, Vec3::ZERO),
            clip: Some((AnimationNodeIndex::new(1), 3.333)),
            armed_at: 10.0,
            window_hi: f32::NEG_INFINITY,
            anim_id: Some(0),
            // Parked, and the clock still answers: honouring the animate set is the caller's job.
            active: false,
            parked_at: 0.0,
        };

        let (_, seek) = host.arm_clock(10.0).expect("armed");
        assert!(seek.abs() < 1e-6, "at the arm it sits at t = 0");

        let (_, seek) = host.arm_clock(12.0).expect("armed");
        assert!((seek - 2.0).abs() < 1e-4, "2 s in, {seek}");

        let (_, seek) = host.arm_clock(14.0).expect("armed");
        assert!((seek - (4.0 - 3.333)).abs() < 1e-4, "wrapped to {seek}");

        host.clip = Some((AnimationNodeIndex::new(1), 0.0));
        let (_, seek) = host.arm_clock(99.0).expect("armed");
        assert!(
            seek.abs() < 1e-6,
            "a zero-length clip sits at its own t = 0"
        );

        host.clip = None;
        assert!(host.arm_clock(12.0).is_none());
    }
}
