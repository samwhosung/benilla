//! Global-sequence bone channels — free-clock loops *independent* of the playing animation (the
//! character eye-blink eyelid scale; resting fidget pulses; a spell effect's star twinkles).
//! benilla's per-sequence reader deliberately drops them: they key off their own global-sequence
//! timer, not the playing sequence's time band (`benilla_formats::parse_m2_animations`). This
//! samples them on the instance's global-sequence clock and writes the driven joint components
//! *after* the [`AnimationPlayer`] posed the skeleton.
//!
//! Ground truth: the cursor is
//! `(sceneClock − instanceAttachTime) % duration` — ONE free-running per-scene ms clock
//! (`[scene+0xc]`, advanced once per scene update), snapshotted ONCE per model instance at attach
//! (`CM2Model+0x68`, written unconditionally at `0x70eae1`). So a fresh instance starts its
//! global sequences at phase 0, and two instances attached on different frames run at different
//! phases — per-INSTANCE anchoring, not per-play arming (sequence tracks re-arm per play; the
//! gseq anchor is stamped once). Spell effects are NOT an exception: the lifecycle is
//! fresh-per-play (`0x707350` alloc→ctor→attach; teardown is a hard free at `0x70e170`, no
//! pooling), and the director's own 3-cast
//! apitrace shows the impact flash at the same +16-frame offset every cast — the apparent
//! cast-to-cast scatter is particle randomness plus the moving cast-anim hands, not clock
//! phase (decisions 0855/0856/0858). The canonical creature consumer is the eyelid:
//! its scale is `0` (lid retracted, eye open) for ~96% of the loop and `1` (lid full, eye shut)
//! for ~100 ms — the blink. Without this pass the eyelid sits at its default identity scale
//! (full size) forever: eyes shut.

use bevy::prelude::*;

use benilla_assets::GlobalBone;

/// One channel's write target: a live joint entity (the doodad/effect/booth lane) or a bone index
/// into the host's [`super::RigPose`] locals (the collapsed unit lane, decision 0724).
enum SeqTarget {
    Joint(Entity),
    Bone(u16),
}

/// Per-instance driver for a model's global-sequence bone channels: each channel's write target
/// paired with its baked channels, plus the instance's clock **anchor** — the attach-time
/// snapshot of the shared scene clock (`CM2Model+0x68`, module docs). Attached beside the
/// [`AnimationPlayer`] on a skinned instance whose model carries any global-sequence track.
#[derive(Component)]
pub struct GlobalSeqDrive {
    /// `(write target, its baked global-sequence channels)`.
    bones: Vec<(SeqTarget, GlobalBone)>,
    /// The attach snapshot of this instance's SCENE clock (secs): `None` until the first animate
    /// tick stamps it (the instance's attach — the ref writes `+0x68` once, at attach).
    anchor: Option<f64>,
    /// **This instance's scene clock**, when it is not the world's (secs). The kernel's Phase B
    /// reads `[[model+0x2c]+0xc]` — the clock of the scene that OWNS the instance — and a
    /// `<Model>` widget owns a private `CM2Scene` at `CSimpleModel+0x314` (`0x76cfc0`), advanced
    /// by the widget's own `OnUpdate` (`0x76d7f0`) and by nothing else. So a UI model tile writes
    /// its pane's clock here every frame it draws and the phase rides the pane, not the world:
    /// a pane whose frame is hidden stops its clock, and the spin resumes where it stopped.
    /// `None` — every world lane — is the world scene's free-running clock (decision 2046).
    clock: Option<f64>,
    /// Paused: skip the joint writes (the doodad host gates animation to drawn instances — the
    /// ref's kernel ticks at draw time (`0x707680`), so a culled model isn't evaluated). Creatures
    /// never pause. Resuming needs no re-seek: the cursor is a pure
    /// function of the shared clock and the attach anchor, so a re-appearing doodad shows the
    /// pose the clock dictates.
    paused: bool,
}

impl GlobalSeqDrive {
    /// Map each of the model's global-sequence bones to this instance's joint entity. `None` when the
    /// model has no global-sequence tracks (the common case) or none resolve to a joint — the entity
    /// then gets no component and the driver skips it.
    pub fn new(global_bones: &[GlobalBone], joints: &[Entity]) -> Option<Self> {
        let bones: Vec<_> = global_bones
            .iter()
            .filter_map(|g| {
                joints
                    .get(g.bone as usize)
                    .map(|&e| (SeqTarget::Joint(e), g.clone()))
            })
            .collect();
        (!bones.is_empty()).then_some(Self {
            bones,
            anchor: None,
            clock: None,
            paused: false,
        })
    }

    /// The collapsed-rig lane (decision 0724): channels write the host's [`super::RigPose`]
    /// locals by bone index — no joint entities exist. Same `None` gate as [`Self::new`].
    pub fn new_rig(global_bones: &[GlobalBone], nbones: usize) -> Option<Self> {
        let bones: Vec<_> = global_bones
            .iter()
            .filter(|g| (g.bone as usize) < nbones)
            .map(|g| (SeqTarget::Bone(g.bone), g.clone()))
            .collect();
        (!bones.is_empty()).then_some(Self {
            bones,
            anchor: None,
            clock: None,
            paused: false,
        })
    }

    /// Pause/resume the joint writes — the doodad draw gate's lever, and its only caller. (The
    /// booth and UI-tile lanes park instead, with [`super::AnimParked`] on the root, which holds
    /// the pose evaluation and the compose as well as these writes; this pause holds only the
    /// writes, which is what the doodad lane wants for a host whose player it also stops.) While
    /// paused the joints hold their last pose; resume lands on the anchored cursor with nothing
    /// to catch up.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Point this instance's channels at its own scene clock (secs) — see [`Self::clock`]. Written
    /// per frame by the owner of a scene that is not the world's; never called by a world lane.
    pub fn set_clock(&mut self, secs: f64) {
        self.clock = Some(secs);
    }
}

/// Sample every drive's channels at its anchored cursor (`sceneNow − anchor`, stamping the
/// anchor on the first tick — the attach) and write the driven bone — in the pose post-pass
/// window ([`super::PosePost`], the same as the body twist), so the model compose folds it. A
/// channel overwrites only its own component; a bone the playing animation never keyed (the
/// eyelid) keeps its rest translation/rotation and takes only the global scale, so the eye opens
/// and blinks over whatever gait is playing. The cursor wraps per channel in f64 (`t % period`)
/// so a long-uptime clock keeps millisecond precision through the f32 sampler.
fn apply_global_sequences(
    time: Res<Time>,
    mut drives: Query<(Entity, &mut GlobalSeqDrive, Has<super::AnimParked>)>,
    mut joints: Query<&mut Transform>,
    mut rigs: Query<&mut super::RigPose>,
) {
    let now = time.elapsed_secs_f64();
    for (host, mut drive, parked) in &mut drives {
        // The clock is the instance's SCENE's — the world's for every world lane, and a `<Model>`
        // widget's private one where the tile renderer wrote it ([`GlobalSeqDrive::clock`]).
        let scene_now = drive.clock.unwrap_or(now);
        // The attach stamp happens even while parked/paused — the ref stamps +0x68 at attach,
        // not at first draw.
        let t = scene_now - *drive.anchor.get_or_insert(scene_now);
        // A parked or paused instance skips only the WRITES — the cursor is absolute
        // (decision 0448's absolute-clock ruling, now literal: nothing per-instance advances).
        if drive.paused || parked {
            continue;
        }
        let mut rig = rigs.get_mut(host).ok();
        for (target, bone) in &drive.bones {
            let tf: &mut Transform = match target {
                SeqTarget::Joint(joint) => {
                    let Ok(tf) = joints.get_mut(*joint) else {
                        continue;
                    };
                    tf.into_inner()
                }
                SeqTarget::Bone(b) => {
                    let Some(rig) = rig.as_mut() else { continue };
                    rig.pose_dirty = true;
                    let Some(tf) = rig.locals.get_mut(*b as usize) else {
                        continue;
                    };
                    tf
                }
            };
            let at = |period: f32| (t % f64::from(period.max(1e-3))) as f32;
            if let Some(c) = &bone.translation {
                tf.translation = c.sample(at(c.period));
            }
            if let Some(c) = &bone.rotation {
                tf.rotation = c.sample(at(c.period));
            }
            if let Some(c) = &bone.scale {
                tf.scale = c.sample(at(c.period));
            }
        }
    }
}

/// Register [`apply_global_sequences`] in the pose post-pass window (beside the body twist).
pub fn plugin(app: &mut App) {
    app.add_systems(PostUpdate, apply_global_sequences.in_set(super::PosePost));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig_anim::RigPose;
    use benilla_assets::GlobalSeqChannel;

    fn eyelid_bone() -> GlobalBone {
        GlobalBone {
            bone: 75,
            translation: None,
            rotation: None,
            // The real eyelid shape: open (0) at the loop start, shut (1) for the blink window, open again.
            scale: Some(GlobalSeqChannel {
                period: 6.633,
                keys: vec![
                    (0.0, Vec3::ZERO),
                    (0.033, Vec3::ONE),
                    (0.100, Vec3::ONE),
                    (0.133, Vec3::ZERO),
                ],
            }),
        }
    }

    /// A linear-ramp channel (`scale = t`, long period) — phase differences are visible at any
    /// absolute elapsed.
    fn ramp() -> GlobalBone {
        GlobalBone {
            bone: 0,
            translation: None,
            rotation: None,
            scale: Some(GlobalSeqChannel {
                period: 100.0,
                keys: vec![(0.0, Vec3::ZERO), (100.0, Vec3::splat(100.0))],
            }),
        }
    }

    /// The anchor law (0856): a FRESH drive stamps its attach on its first tick, so a drive
    /// spawned later reads a SMALLER cursor than one spawned earlier (per-instance phase — two
    /// creatures attached on different frames blink at different times). Ticks are
    /// deterministic via `TimeUpdateStrategy::ManualDuration`.
    #[test]
    fn fresh_drives_anchor_at_attach() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_millis(60),
        ));
        app.add_systems(Update, apply_global_sequences);

        let early_joint = app.world_mut().spawn(Transform::default()).id();
        app.world_mut()
            .spawn(GlobalSeqDrive::new(&[ramp()], &[early_joint]).expect("a keyed channel maps"));
        app.update();
        app.update();
        // A drive attached two ticks later.
        let late_joint = app.world_mut().spawn(Transform::default()).id();
        app.world_mut()
            .spawn(GlobalSeqDrive::new(&[ramp()], &[late_joint]).expect("a keyed channel maps"));
        app.update();
        app.update();

        let s = |e: Entity| app.world().entity(e).get::<Transform>().unwrap().scale.x;
        assert!(
            s(early_joint) > s(late_joint),
            "the later attach reads a smaller cursor (per-instance anchor): early {} vs late {}",
            s(early_joint),
            s(late_joint)
        );
        assert!(
            s(late_joint) > 0.0,
            "the late drive did tick from its own anchor (got {})",
            s(late_joint)
        );
    }

    /// The eyelid channel itself still samples correctly at a given cursor — the blink window
    /// reads shut, the long tail reads open (the channel sampler is untouched by the anchor
    /// work; only who supplies `t` changed).
    #[test]
    fn eyelid_channel_samples_by_clock_value() {
        let bone = eyelid_bone();
        let c = bone.scale.as_ref().unwrap();
        assert!(
            c.sample(0.06).abs_diff_eq(Vec3::ONE, 1e-3),
            "shut mid-blink"
        );
        assert!(
            c.sample(3.0).abs_diff_eq(Vec3::ZERO, 1e-3),
            "open in the tail"
        );
        assert!(
            c.sample(3.0 + 2.0 * 6.633).abs_diff_eq(Vec3::ZERO, 1e-3),
            "wraps on its period"
        );
    }

    /// The two write targets of the one sampler agree (decision 1360's second golden, built
    /// AHEAD of the doodad collapse): the same channels driven through a joint entity
    /// ([`GlobalSeqDrive::new`], the doodad/effect lane) and through a `RigPose` local
    /// ([`GlobalSeqDrive::new_rig`], the collapsed lane) read **bit-identically** frame after
    /// frame — the collapse changes where a sample lands, never what it is. Both drives spawn
    /// the same frame, so the per-instance anchors coincide by the attach law.
    #[test]
    fn joint_and_rig_targets_write_the_same_pose() {
        let full = GlobalBone {
            bone: 0,
            translation: Some(benilla_assets::GlobalSeqChannel {
                period: 2.5,
                keys: vec![(0.0, Vec3::ZERO), (1.2, Vec3::X), (2.5, Vec3::NEG_Z)],
            }),
            rotation: Some(benilla_assets::GlobalSeqChannel {
                period: 1.7,
                keys: vec![(0.0, Quat::IDENTITY), (1.7, Quat::from_rotation_y(0.9))],
            }),
            scale: Some(benilla_assets::GlobalSeqChannel {
                period: 100.0,
                keys: vec![(0.0, Vec3::ONE), (100.0, Vec3::splat(3.0))],
            }),
        };
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_millis(47),
        ));
        app.add_systems(Update, apply_global_sequences);

        let joint = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().spawn(
            GlobalSeqDrive::new(std::slice::from_ref(&full), &[joint]).expect("keyed channels map"),
        );
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
        let host = app.world_mut().spawn_empty().id();
        app.world_mut().entity_mut(host).insert((
            RigPose::new(host, &skeleton),
            GlobalSeqDrive::new_rig(&[full], 1).expect("keyed channels map"),
        ));

        for frame in 0..6 {
            app.update();
            let jt = *app.world().entity(joint).get::<Transform>().unwrap();
            let rl = app.world().entity(host).get::<RigPose>().unwrap().locals[0];
            assert_eq!(jt, rl, "frame {frame}: joint target vs rig target diverged");
        }
    }
}
