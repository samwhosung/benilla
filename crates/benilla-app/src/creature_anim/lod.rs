//! The animation-LOD gate: a rig off the frustum, or in a room the portal PVS does not reach,
//! parks its pose evaluation ([`AnimParked`]) while its clocks run on; every [`RigPose`] is
//! covered, GameObject rigs included. The reference elects every scene object each frame
//! (`0x683dd0`): a pass-2 unit is neither drawn, animated nor event-ticked, except a
//! `MORE_AUDIBLE` creature, re-linked for the tick (`0x607da0`). So a parked rig fires no event
//! keys unless flagged, and wakes to the pose its absolute clock dictates (`0x714260` samples the
//! world clock). Unlike the doodad gate ([`benilla_world::doodad_anim`]) this never stops the
//! [`AnimationPlayer`], whose clock the driver and the event scan still use.
//!
//! The room leg parks a rig whose WMO room ([`UnitWmoRoom`]) and every room one portal hop away
//! are outside the PVS: a body pokes out of its room only through a portal. The PVS is built in
//! `Update`, before this gate, so a reveal wakes the same frame; every lookup seam fails open.

use bevy::app::AnimationSystems;
use bevy::camera::primitives::{Frustum, Sphere as CullSphere};
use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use benilla_assets::WmoModel;

use crate::entities::mount::MountBody;
use crate::net::Embodied;
use crate::target::SelectionRadius;
use benilla_world::view::WorldCamera;
use benilla_world::wmo_portal::{room_pvs_visible, UnitWmoRoom, WmoPortalInstance};

use benilla_world::rig_anim::{AnimParked, RigPose};

use crate::portrait::StageRig;

/// Time out of view before a rig parks, so a camera swing does not churn; waking is instant.
const PARK_AFTER_SECS: f32 = 0.5;

/// The frustum sphere, `SelectionRadius × root scale × RADIUS_SCALE + RADIUS_PAD`, keeps any rig
/// with a pixel on screen inside, past swings, attachments and the one-frame-old frustum; a
/// model-less unit takes [`FALLBACK_RADIUS`]. Too large only keeps more rigs live.
const RADIUS_SCALE: f32 = 2.0;
const RADIUS_PAD: f32 = 4.0;
const FALLBACK_RADIUS: f32 = 6.0;

/// Park or wake each rig by its padded frustum sphere and its room, before [`AnimationSystems`] so
/// a wake samples the same frame. The body we drive and its mount never park: the camera rides its
/// attachment 17 and it animates faded out in first person. `WOW_NO_ANIM_LOD=1` never parks and
/// `WOW_NO_ROOM_LOD=1` drops the room leg.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(super) fn gate_rig_animation(
    time: Res<Time>,
    cam: Query<&Frustum, With<WorldCamera>>,
    rigs: Query<
        (
            Entity,
            &GlobalTransform,
            Option<&SelectionRadius>,
            Has<AnimParked>,
            Has<Embodied>,
            Option<&MountBody>,
            Option<&UnitWmoRoom>,
        ),
        // Every rig, driver or not: a GameObject rig's looping player is its whole animation.
        // Doodads and booth stages are left out: their own gates own their `AnimParked`, and two
        // writers to one marker would fight (a booth stage is outside every world frustum).
        (
            With<RigPose>,
            Without<benilla_world::doodad_anim::DoodadAnimHost>,
            Without<StageRig>,
        ),
    >,
    self_hosts: Query<Has<Embodied>>,
    instances: Query<&WmoPortalInstance>,
    wmos: Res<Assets<WmoModel>>,
    mut out_since: Local<EntityHashMap<f32>>,
    mut disabled: Local<Option<bool>>,
    mut park_all: Local<Option<bool>>,
    mut no_room: Local<Option<bool>>,
    mut commands: Commands,
) {
    let disabled = *disabled.get_or_insert_with(|| std::env::var_os("WOW_NO_ANIM_LOD").is_some());
    // `WOW_ANIM_PARK_ALL` parks every rig, visible ones too: the pose lane's cost floor, as
    // `WOW_NO_ANIM_LOD` is its ceiling. A measuring lever only.
    let park_all = *park_all.get_or_insert_with(|| std::env::var_os("WOW_ANIM_PARK_ALL").is_some());
    let no_room = *no_room.get_or_insert_with(|| std::env::var_os("WOW_NO_ROOM_LOD").is_some());
    // The `[rig-gate]` counters (`WOW_RIG_COST`): `room_out` counts the rigs the frustum keeps and
    // the PVS rejects, even under `WOW_NO_ROOM_LOD`.
    let cost_on = benilla_world::rig_palette::rig_cost_enabled();
    let (mut n_rigs, mut n_parked, mut room_out) = (0u32, 0u32, 0u32);
    let Ok(frustum) = cam.single() else {
        return;
    };
    let now = time.elapsed_secs();
    for (entity, tf, radius, parked, is_self, mount, room) in &rigs {
        let exempt =
            disabled || is_self || mount.is_some_and(|m| self_hosts.get(m.host).unwrap_or(false));
        let visible = exempt
            || !park_all && {
                let scale = tf.to_scale_rotation_translation().0.max_element();
                let r = radius.map_or(FALLBACK_RADIUS, |r| r.0 * scale * RADIUS_SCALE + RADIUS_PAD);
                let sphere_in = frustum.intersects_sphere(
                    &CullSphere {
                        center: tf.translation().into(),
                        radius: r,
                    },
                    false,
                );
                // The room leg: only a rig the frustum leg kept can be room-parked.
                let room_in = !sphere_in || (no_room && !cost_on) || {
                    let ok = room_pvs_visible(room, &instances, &wmos);
                    if !ok {
                        room_out += 1;
                    }
                    ok || no_room
                };
                sphere_in && room_in
            };
        if cost_on {
            n_rigs += 1;
            n_parked += u32::from(parked);
        }
        if visible {
            out_since.remove(&entity);
            if parked {
                commands.entity(entity).remove::<AnimParked>();
            }
        } else if !parked {
            let since = *out_since.entry(entity).or_insert(now);
            if now - since >= PARK_AFTER_SECS {
                out_since.remove(&entity);
                commands.entity(entity).insert(AnimParked);
            }
        }
    }
    if cost_on {
        eprintln!("[rig-gate] rigs={n_rigs} parked={n_parked} room_out={room_out}");
    }
    // Drop entries for rigs that despawned (or lost their model) before parking.
    out_since.retain(|e, _| rigs.contains(*e));
}

/// Register [`gate_rig_animation`] before the frame's pose evaluation.
pub(super) fn plugin(app: &mut App) {
    app.add_systems(PostUpdate, gate_rig_animation.before(AnimationSystems));
}

#[cfg(test)]
mod tests {
    use bevy::animation::animation_curves::{AnimatableCurve, AnimatableKeyframeCurve};
    use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle};
    use bevy::animation::{animated_field, AnimationClip};

    use benilla_assets::{
        bone_target_id, AnimClip, ModelAnimations, PoseBone, PoseClip, PoseSource, PoseTrack,
        WmoGroupNav, WmoPortalRef,
    };
    use benilla_world::wmo_portal::WmoRoom;

    use super::super::{events::fire_anim_events, AnimDriver, AnimSoundEvent};
    use super::*;

    /// The test clip's loop length in seconds, short so real frames wrap it several times.
    const DUR: f32 = 0.4;

    /// A one-joint rig looping bone 0 from the origin to `+X` over [`DUR`], with a `$TST` key
    /// mid-loop. Returns `(root, joint)`.
    fn spawn_rig(app: &mut App, at: Vec3) -> (Entity, Entity) {
        let mut pose = PoseSource {
            bone_masks: vec![0],
            ..Default::default()
        };
        let mut pose_clip = PoseClip::default();
        pose_clip.push(PoseBone {
            bone: 0,
            translation: PoseTrack::new(&[(0.0, Vec3::ZERO), (DUR, Vec3::X)]),
            rotation: PoseTrack::default(),
            scale: PoseTrack::default(),
        });
        pose.clips.push(pose_clip);
        let mut clip = AnimationClip::default();
        clip.add_curve_to_target(
            bone_target_id(0),
            AnimatableCurve::new(
                animated_field!(Transform::translation),
                AnimatableKeyframeCurve::new([(0.0, Vec3::ZERO), (DUR, Vec3::X)])
                    .expect("two keyframes build"),
            ),
        );
        let clip_handle = app
            .world_mut()
            .resource_mut::<Assets<AnimationClip>>()
            .add(clip);
        let (graph, node) = AnimationGraph::from_clip(clip_handle);
        pose.set_node(node, 0, 0);
        let graph_handle = app
            .world_mut()
            .resource_mut::<Assets<AnimationGraph>>()
            .add(graph);

        let mut player = AnimationPlayer::default();
        player.play(node).repeat();
        let anims = ModelAnimations {
            graph: graph_handle.clone(),
            clips: vec![AnimClip {
                anim_id: 0,
                seq_index: 0,
                node,
                looping: true,
                duration: DUR,
                move_speed: 0.0,
                blend_time: 0.0,
                bounds_center: Vec3::ZERO,
                bounds_radius: 0.0,
                bounds_min: Vec3::ZERO,
                bounds_max: Vec3::ZERO,
                events: vec![benilla_assets::ClipEvent {
                    time: DUR * 0.5,
                    ident: *b"$TST",
                    data: 0,
                    bone: 0,
                    offset: Vec3::ZERO,
                    point: Vec3::ZERO,
                }]
                .into(),
                arm_nodes: None,
                upper_node: None,
                frequency: 0,
                replay: (0, 0),
                poses_bones: true,
            }],
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: std::sync::Arc::new(pose),
        };
        // The gait slot points at the clip, so the event scanner finds its timeline.
        let driver = AnimDriver {
            gait: Some(0),
            ..Default::default()
        };
        let root = app
            .world_mut()
            .spawn((
                Transform::from_translation(at),
                GlobalTransform::from_translation(at),
                player,
                AnimationGraphHandle(graph_handle),
                anims,
                driver,
            ))
            .id();
        // The evaluator's pose buffer; no joint entities.
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
        app.world_mut()
            .entity_mut(root)
            .insert(benilla_world::rig_anim::RigPose::new(root, &skeleton));
        (root, root)
    }

    /// A camera at the origin looking down −Z (view = identity): rigs at −Z are in frame, rigs
    /// at +Z are behind it.
    fn spawn_camera(app: &mut App) {
        let clip_from_world = Mat4::perspective_rh(1.2, 1.0, 0.1, 1000.0);
        app.world_mut()
            .spawn((WorldCamera, Frustum::from_clip_from_world(&clip_from_world)));
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            bevy::animation::AnimationPlugin,
        ));
        app.init_asset::<WmoModel>();
        app.add_message::<AnimSoundEvent>();
        // Empty: every parked rig reads unflagged.
        app.init_resource::<crate::names::NameCache>();
        // The live order: the gate, the pose evaluation, then the event scan.
        app.add_systems(PostUpdate, gate_rig_animation.before(AnimationSystems));
        benilla_world::rig_anim::plugin(&mut app);
        app.add_systems(PostUpdate, fire_anim_events.after(AnimationSystems));
        app
    }

    /// Step about `secs` of wall-clock time across `steps` updates.
    fn advance(app: &mut App, secs: f32, steps: u32) {
        for _ in 0..steps {
            std::thread::sleep(std::time::Duration::from_secs_f32(secs / steps as f32));
            app.update();
        }
    }

    fn seek(app: &App, rig: Entity) -> f32 {
        let player = app.world().entity(rig).get::<AnimationPlayer>().unwrap();
        let (_, active) = player.playing_animations().next().expect("clip armed");
        active.seek_time()
    }

    fn bone(app: &App, rig: Entity) -> Vec3 {
        app.world()
            .entity(rig)
            .get::<benilla_world::rig_anim::RigPose>()
            .unwrap()
            .locals[0]
            .translation
    }

    /// An off-frustum rig parks with its bone frozen, its clock running and its keys silent, as
    /// pass 2 never ticks an unflagged model (`0x683dd0`); woken, it matches a never-parked twin.
    #[test]
    fn parked_rig_keeps_its_clock_falls_silent_and_wakes_to_the_absolute_pose() {
        let mut app = app();
        spawn_camera(&mut app);
        // `behind` starts off-frustum (+Z, behind the camera); `twin` stays in frame at −Z.
        let (behind, behind_joint) = spawn_rig(&mut app, Vec3::Z * 50.0);
        let (twin, twin_joint) = spawn_rig(&mut app, -Vec3::Z * 20.0);
        app.update();

        // Past the hysteresis window: parked.
        advance(&mut app, PARK_AFTER_SECS + 0.2, 4);
        assert!(
            app.world().entity(behind).contains::<AnimParked>(),
            "off-frustum past the hysteresis window ⇒ parked"
        );
        assert!(
            !app.world().entity(twin).contains::<AnimParked>(),
            "the in-frame twin stays live"
        );
        // While parked the bone holds, the clock runs and the event track is not scanned.
        let frozen = bone(&app, behind_joint);
        let seek_at_park = seek(&app, behind);
        let mut cursor = app
            .world()
            .resource::<Messages<AnimSoundEvent>>()
            .get_cursor_current();
        let (mut fired_parked, mut fired_twin) = (0usize, 0usize);
        for _ in 0..6 {
            // More than one loop in total, so the key is crossed whatever the phase.
            std::thread::sleep(std::time::Duration::from_secs_f32(DUR * 0.3));
            app.update();
            let messages = app.world().resource::<Messages<AnimSoundEvent>>();
            for ev in cursor.read(messages).filter(|ev| &ev.ident == b"$TST") {
                fired_parked += usize::from(ev.entity == behind);
                fired_twin += usize::from(ev.entity == twin);
            }
        }
        assert_eq!(
            fired_parked, 0,
            "an unflagged parked rig's event keyframes are not scanned (1482)"
        );
        assert!(fired_twin > 0, "the live twin keeps firing — the control");
        assert_eq!(
            frozen,
            bone(&app, behind_joint),
            "a parked bone's Transform never changes"
        );
        assert_ne!(
            seek_at_park,
            seek(&app, behind),
            "the parked rig's seek clock keeps advancing"
        );

        // Wake in front of the camera: the same frame, seek and bone equal the twin's (both
        // players were armed the same frame and tick the same deltas).
        app.world_mut()
            .entity_mut(behind)
            .insert(GlobalTransform::from_translation(-Vec3::Z * 20.0));
        app.update();
        assert!(
            !app.world().entity(behind).contains::<AnimParked>(),
            "in-frustum ⇒ instant wake"
        );
        assert_eq!(
            seek(&app, behind),
            seek(&app, twin),
            "the seek clocks never diverged"
        );
        assert_eq!(
            bone(&app, behind_joint),
            bone(&app, twin_joint),
            "the woken pose equals the never-parked twin's — the absolute-clock snap"
        );
    }

    /// A parked creature whose template carries `MORE_AUDIBLE` (`0x20`) keeps firing: the
    /// reference's pass-2 re-link (`0x607da0`).
    #[test]
    fn a_more_audible_parked_rig_keeps_firing() {
        let mut app = app();
        spawn_camera(&mut app);
        let (behind, _) = spawn_rig(&mut app, Vec3::Z * 50.0);
        // The gate keys on `OBJECT_FIELD_ENTRY` (field 3), as the reference keys its
        // creature-query record, not on the guid's entry.
        const ENTRY: u32 = 69;
        app.world_mut()
            .entity_mut(behind)
            .insert(crate::net::ObjectStore(
                benilla_protocol::ObjectFields::from_pairs(&[(3, ENTRY)]),
            ));
        app.world_mut()
            .resource_mut::<crate::names::NameCache>()
            .insert_creature(
                ENTRY,
                Some(crate::names::CreatureRecord {
                    name: "Growler".into(),
                    subname: None,
                    creature_type: 1,
                    pet_family: 0,
                    rank: 0,
                    type_flags: crate::names::type_flags::MORE_AUDIBLE,
                    civilian: false,
                    racial_leader: false,
                    display_id: 0,
                }),
            );
        app.update();
        advance(&mut app, PARK_AFTER_SECS + 0.2, 4);
        assert!(
            app.world().entity(behind).contains::<AnimParked>(),
            "flagged or not, the POSE still parks — the flag is tick-only"
        );
        let mut cursor = app
            .world()
            .resource::<Messages<AnimSoundEvent>>()
            .get_cursor_current();
        let mut fired = 0usize;
        for _ in 0..6 {
            std::thread::sleep(std::time::Duration::from_secs_f32(DUR * 0.3));
            app.update();
            let messages = app.world().resource::<Messages<AnimSoundEvent>>();
            fired += cursor
                .read(messages)
                .filter(|ev| ev.entity == behind && &ev.ident == b"$TST")
                .count();
        }
        assert!(
            fired > 0,
            "MORE_AUDIBLE keeps the off-screen event track scanned"
        );
    }

    /// A staged rig (a booth doll, outside every world frustum) is never this gate's to park: the
    /// booth camera gate owns its `AnimParked`.
    #[test]
    fn a_stage_rig_is_never_world_parked() {
        let mut app = app();
        spawn_camera(&mut app);
        let (stage, _) = spawn_rig(&mut app, Vec3::Z * 50.0); // behind the camera, like any stage
        app.world_mut()
            .entity_mut(stage)
            .insert(crate::portrait::StageRig);
        app.update();
        advance(&mut app, PARK_AFTER_SECS + 0.2, 4);
        assert!(
            !app.world().entity(stage).contains::<AnimParked>(),
            "an off-frustum stage rig stays live — its park is the booth gate's, not the world's"
        );
    }

    /// A [`WmoGroupNav`] with only its portal-ref slice set.
    fn nav(ref_start: u16, ref_count: u16) -> WmoGroupNav {
        WmoGroupNav {
            flags: 0,
            bbox_min: [0.0; 3],
            bbox_max: [0.0; 3],
            ref_start,
            ref_count,
            area_table_id: 0,
            fog_indices: [0; 4],
            group_liquid: benilla_formats::NO_GROUP_LIQUID,
        }
    }

    fn pref(group: u16) -> WmoPortalRef {
        WmoPortalRef {
            portal: 0,
            group,
            side: 1,
        }
    }

    /// An in-frustum rig whose room and its one hop are dark parks; the room's bit wakes it.
    #[test]
    fn a_room_dark_rig_parks_and_wakes_when_the_pvs_reaches_it() {
        let mut app = app();
        spawn_camera(&mut app);
        // In front of the camera: only the room leg can park it.
        let (rig, joint) = spawn_rig(&mut app, -Vec3::Z * 20.0);
        let model = WmoModel {
            wmo_id: 1,
            portal_refs: vec![pref(1), pref(0)],
            group_nav: vec![nav(0, 1), nav(1, 1)],
            ..Default::default()
        };
        let handle = app
            .world_mut()
            .resource_mut::<Assets<WmoModel>>()
            .add(model);
        let inst = app
            .world_mut()
            .spawn(WmoPortalInstance {
                handle,
                world_from_local: bevy::math::Affine3A::IDENTITY,
                name_set: 0,
                visible: vec![false, false],
                interior_fog: vec![false, false],
                liquid_visited: vec![false, false],
                flooded: vec![None, None],
            })
            .id();
        app.world_mut()
            .entity_mut(rig)
            .insert(UnitWmoRoom::claimed(WmoRoom {
                instance: inst,
                group: 0,
            }));
        advance(&mut app, PARK_AFTER_SECS + 0.2, 4);
        assert!(
            app.world().entity(rig).contains::<AnimParked>(),
            "in-frustum but room-dark past the hysteresis window ⇒ parked"
        );
        let frozen = bone(&app, joint);
        // The flood reaches the room: wake and sample.
        app.world_mut()
            .entity_mut(inst)
            .get_mut::<WmoPortalInstance>()
            .unwrap()
            .visible[0] = true;
        advance(&mut app, DUR * 0.3, 2);
        assert!(
            !app.world().entity(rig).contains::<AnimParked>(),
            "PVS reach ⇒ instant wake"
        );
        assert_ne!(
            frozen,
            bone(&app, joint),
            "the woken rig's bones sample again"
        );
    }

    /// A driverless rig (a GameObject's) parks too: the gate keys on `RigPose`, not `AnimDriver`.
    #[test]
    fn a_driverless_go_rig_parks_too() {
        let mut app = app();
        spawn_camera(&mut app);
        let (behind, joint) = spawn_rig(&mut app, Vec3::Z * 50.0);
        app.world_mut().entity_mut(behind).remove::<AnimDriver>();
        advance(&mut app, PARK_AFTER_SECS + 0.2, 4);
        assert!(
            app.world().entity(behind).contains::<AnimParked>(),
            "no driver, still parked — the gate keys on RigPose"
        );
        let frozen = bone(&app, joint);
        advance(&mut app, DUR * 0.35, 2);
        assert_eq!(frozen, bone(&app, joint), "the parked GO rig's bones hold");
    }

    #[test]
    fn the_self_avatar_is_exempt() {
        let mut app = app();
        spawn_camera(&mut app);
        let (behind, behind_joint) = spawn_rig(&mut app, Vec3::Z * 50.0);
        app.world_mut().entity_mut(behind).insert(Embodied);
        advance(&mut app, PARK_AFTER_SECS + 0.2, 4);
        assert!(
            !app.world().entity(behind).contains::<AnimParked>(),
            "the active mover never parks, wherever the camera points"
        );
        // Its bones keep animating: the exemption is live evaluation, not just the marker.
        let before = bone(&app, behind_joint);
        advance(&mut app, DUR * 0.35, 2);
        assert_ne!(before, bone(&app, behind_joint), "its joints stay live");
    }
}
