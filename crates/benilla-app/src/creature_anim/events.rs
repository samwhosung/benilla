//! The model event-keyframe scanner: [`fire_anim_events`] fires the M2 `$xxx` keys each playing
//! clip crossed this frame as [`AnimSoundEvent`]s.

use benilla_assets::{AnimClip, ModelAnimations};
use bevy::animation::graph::AnimationNodeIndex;
use bevy::prelude::*;

use super::{resolved_id, AnimData, AnimDriver};
use crate::names::type_flags::MORE_AUDIBLE;

/// A model event keyframe (an M2 `$xxx` tag) crossed during playback this frame.
#[derive(Message, Clone, Copy)]
pub(crate) struct AnimSoundEvent {
    pub(crate) entity: Entity,
    /// The forward-stored 4CC tag (`*b"$FL0"`, `*b"$SND"`, …).
    pub(crate) ident: [u8; 4],
    /// The tag payload (a SoundEntries id for `$SND`/`$DSL`/`$DSO`; 0 otherwise).
    pub(crate) data: u32,
    /// The `AnimationData.dbc` id of the clip that fired the key: the reference's `0x5fdb50`
    /// answer when a handler runs, which the `$BWR` bow/rifle fork reads (`0x5fcfb0` bow
    /// {46,105,109}, `0x5fcfd0` rifle {49,106,110}).
    pub(crate) anim_id: u16,
    /// Where the key fired, `placementMatrix · (boneMatrix[bone] · position)`, snapshotted at the
    /// fire as the event kernel `0x719370` does for every dispatcher (`0x5ffbd0` units,
    /// `0x5f3e20` GameObjects, `0x6951e0` placed models). Not the object's origin: 149 of 244
    /// shipped `$DSL` keys sit off it, up to 67.6 yd. `None` uses the object's own transform.
    pub(crate) pos: Option<Vec3>,
}

/// The frame a fired key's world point resolves in: a rigged model composes the key's bone-local
/// offset through that bone's live joint; a rig-less one has only identity bones, so
/// `placement · point` is the kernel's value exactly.
pub(crate) struct EventFrame<'a> {
    /// The model's world frame, the reference's placement matrix.
    pub(crate) world: &'a GlobalTransform,
    /// The composed pose and its joints root, for a model that has a rig.
    pub(crate) rig: Option<(&'a benilla_world::rig_anim::RigPose, &'a GlobalTransform)>,
}

impl EventFrame<'_> {
    /// The world point for one baked key.
    fn point(&self, e: &benilla_assets::ClipEvent) -> Vec3 {
        self.rig
            .and_then(|(pose, root)| pose.posed_point(root, e.bone, e.offset))
            .unwrap_or_else(|| self.world.transform_point(e.point))
    }
}

/// The footstep sound channel: `$FSD` alone, which the dispatcher `0x5ffbd0` routes to
/// `0x623390`; the per-foot side tags go to the visual handler `0x5fbf70` and never sound.
pub(crate) fn is_footstep_sound(ident: &[u8; 4]) -> bool {
    ident == b"$FSD"
}

/// The footfall handler's radius, 2500 yd² (50 yd, `[0x80c5b4]` read at `0x5fc00a` in
/// `0x5fbf70`), measured from the active camera's eye (`0x4818f0`, `[[0xb4b2bc]+0x65b8]`). It
/// gates all the handler spawns (decal, camera shake, spray), the local player's feet included:
/// the `GUID == local player` compare (`0x5fc042`–`0x5fc06b`) only picks the decal's ring pool.
const FOOTFALL_RADIUS_SQ: f32 = 2500.0;

/// Whether a footfall is out of the handler's range. The reference sums in x87 extended and
/// rounds once to f32 (`0x5fbffe` store, `0x5fc007` reload); f64 holds every difference, square
/// and sum exactly, so this matches bit for bit where `Vec3::distance_squared` can round across
/// the 50 yd edge. The compare is strict and keeps a NaN (`test ah,0x41; je`), as `>` does.
pub(crate) fn footfall_culls(a: Vec3, b: Vec3) -> bool {
    footfall_dist2(a, b) > FOOTFALL_RADIUS_SQ
}

/// The gate's `dist²`, rounded to f32 once.
fn footfall_dist2(a: Vec3, b: Vec3) -> f32 {
    let d = |p: f32, q: f32| p as f64 - q as f64;
    let (dx, dy, dz) = (d(a.x, b.x), d(a.y, b.y), d(a.z, b.z));
    (dx * dx + dy * dy + dz * dz) as f32
}

/// The visual footfall channel: the foot side a per-foot tag names (`$FL0` gives `L`), over the
/// ten families the dispatcher tables (`0x5ffe32` left, `0x5ffc82` right).
pub(crate) fn footfall_side(ident: &[u8; 4]) -> Option<u8> {
    matches!(
        &ident[..3],
        b"$FL" | b"$FR" | b"$RL" | b"$RR" | b"$SL" | b"$SR" | b"$BL" | b"$BR" | b"$WL" | b"$WR"
    )
    .then_some(ident[2])
}

/// How far (s) into its clip an arm may land and still open the next window at the head: above
/// a hitchy frame, below the corpse settle's `seek_to(duration)` (every Death clip is over 1 s).
const FRESH_CLIP_HEAD: f32 = 0.25;

/// One scanned track's memory: the playing node, its last seek, and whether that seek is the arm.
#[derive(Clone, Copy)]
pub(crate) struct TrackSeek {
    node: AnimationNodeIndex,
    seek: f32,
    armed: bool,
}

/// A scanner's per-entity track memory, the `Local` each [`advance_track`] caller owns.
pub(crate) type TrackMemory = bevy::ecs::entity::EntityHashMap<TrackSeek>;

/// Fire the event keys each unit's base and overlay clips crossed since last frame; runs after
/// [`super::driver::drive_animations`]. A `t = 0` key re-fires on every loop wrap, as in the
/// reference's walker `0x719370`.
#[allow(clippy::type_complexity)] // one Bevy query tuple, the house convention
pub(super) fn fire_anim_events(
    units: Query<(
        Entity,
        &ModelAnimations,
        &AnimationPlayer,
        &AnimDriver,
        Has<benilla_world::rig_anim::AnimParked>,
        // Read only for `OBJECT_FIELD_ENTRY`, the key of the cached creature template.
        Option<&crate::net::ObjectStore>,
        &GlobalTransform,
        Option<&benilla_world::rig_anim::RigPose>,
    )>,
    // The joint roots the composed poses hang off.
    globals: Query<&GlobalTransform>,
    mut last: Local<TrackMemory>,
    // The masked overlay's own memory: a swing or emote there plays beside the base, so its keys
    // are scanned on their own node.
    mut last_overlay: Local<TrackMemory>,
    mut out: MessageWriter<AnimSoundEvent>,
    anim_data: Option<Res<AnimData>>,
    names: Res<crate::names::NameCache>,
) {
    let catalog = anim_data.as_deref().map(|d| &d.0);
    for (entity, anims, player, drv, parked, store, world, pose) in &units {
        // A parked unit's tracks are not scanned, as the reference's second walk never ticks it
        // (`0x683dd0` skips `0x710b90`), unless its template carries `MORE_AUDIBLE` (the
        // `0x607da0` re-link); a missing template reads not audible (`0x623b70`). Dropping the
        // memories makes a waking track re-arm rather than scan the whole parked gap.
        if parked
            && !store
                .and_then(|s| s.0.object_entry())
                .and_then(|entry| names.creature_record(entry))
                .is_some_and(|r| r.type_flags & MORE_AUDIBLE != 0)
        {
            last.remove(&entity);
            last_overlay.remove(&entity);
            continue;
        }
        // Base track: the resolved id's playing variation, each variation its own node and event
        // track; in a same-id cross-fade the newest play (the smallest seek) is scanned.
        let frame = EventFrame {
            world,
            rig: pose.and_then(|p| Some((p, globals.get(p.joints_root).ok()?))),
        };
        if let Some(id) = drv.resolved_anim(anims, catalog) {
            let playing = anims
                .clips
                .iter()
                .filter(|c| c.anim_id == id)
                .filter_map(|c| player.animation(c.node).map(|a| (c, a.seek_time())))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((clip, cur)) = playing {
                if let Some(prev) = advance_track(&mut last, entity, clip.node, cur) {
                    scan_events(clip, entity, prev, cur, &frame, &mut out);
                }
            }
        }
        // Masked overlay track: its node matched back to the variation whose `upper_node` it is.
        if let Some(ov) = drv.overlay {
            let id = resolved_id(anims, ov.id, catalog);
            let clip = anims
                .clips
                .iter()
                .filter(|c| c.anim_id == id)
                .find(|c| c.upper_node == Some(ov.node));
            if let Some(clip) = clip {
                if let Some(active) = player.animation(ov.node) {
                    let cur = active.seek_time();
                    if let Some(prev) = advance_track(&mut last_overlay, entity, ov.node, cur) {
                        scan_events(clip, entity, prev, cur, &frame, &mut out);
                    }
                }
            }
        }
    }
}

/// Advance a track's memory, keyed by graph node, and return the seek to scan from, or `None` on
/// an arm frame. The reference's arm `0x7121a0` stamps the window start at now (`0x712758`), so
/// the walker skips that frame (`0x719518`) and opens the next at the stamp, lower-inclusive
/// (`0x7196d5`–`0x7196d9`): `t = 0` keys fire only for a clip still armed a frame later. An arm
/// deep in its timeline (past [`FRESH_CLIP_HEAD`]) opens at its own stamp, not the head.
pub(crate) fn advance_track(
    last: &mut TrackMemory,
    entity: Entity,
    node: AnimationNodeIndex,
    cur: f32,
) -> Option<f32> {
    let was = last.get(&entity).copied().filter(|t| t.node == node);
    last.insert(
        entity,
        TrackSeek {
            node,
            seek: cur,
            armed: was.is_none(),
        },
    );
    let was = was?; // the arm frame itself: recorded, scanned never
    Some(if was.armed && was.seek <= FRESH_CLIP_HEAD {
        -1.0
    } else {
        was.seek
    })
}

/// Fire the keys `clip` crossed on `(prev, cur]`; a loop wrap (`cur < prev`) fires the tail
/// `(prev, duration]` then the head `[0, cur]`. Every fired key is traced under the `aev` tag
/// (`WOW_MOVE_TRACE`, `WOW_MOVE_TRACE_TAGS=aev`), on the clock of the mover and wire traces.
pub(crate) fn scan_events(
    clip: &AnimClip,
    entity: Entity,
    prev: f32,
    cur: f32,
    frame: &EventFrame<'_>,
    out: &mut MessageWriter<AnimSoundEvent>,
) {
    if clip.events.is_empty() || cur == prev {
        return;
    }
    let traced = benilla_assets::trace::enabled_for("aev");
    let mut fire = |lo: f32, hi: f32| {
        for e in clip.events.iter() {
            if e.time > lo && e.time <= hi {
                let pos = frame.point(e);
                if traced {
                    benilla_assets::trace::line(
                        "aev",
                        &format!(
                            "{} unit={entity} anim={} key={:.3}s data={} clip={:.3}s{} \
                             bone={} at=[{:.2},{:.2},{:.2}] off={:.2}",
                            String::from_utf8_lossy(&e.ident),
                            clip.anim_id,
                            e.time,
                            e.data,
                            clip.duration,
                            if clip.looping { " loop" } else { "" },
                            e.bone,
                            pos.x,
                            pos.y,
                            pos.z,
                            // Distance from the model's origin; 0 is the model-root fallback.
                            pos.distance(frame.world.translation()),
                        ),
                    );
                }
                out.write(AnimSoundEvent {
                    entity,
                    ident: e.ident,
                    data: e.data,
                    anim_id: clip.anim_id,
                    pos: Some(pos),
                });
            }
        }
    };
    if cur >= prev {
        fire(prev, cur);
    } else {
        fire(prev, clip.duration + 1.0);
        fire(-1.0, cur);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_footfall_gate_edge_and_nan_both_pass() {
        let at = |d: f32| Vec3::new(d, 0.0, 0.0);
        assert_eq!(
            footfall_dist2(Vec3::ZERO, at(50.0)),
            FOOTFALL_RADIUS_SQ,
            "50 yd is exactly the threshold"
        );
        assert!(
            !footfall_culls(Vec3::ZERO, at(50.0)),
            "and the edge is kept"
        );
        assert!(footfall_culls(Vec3::ZERO, at(50.001)));
        assert!(
            !footfall_culls(Vec3::ZERO, Vec3::splat(f32::NAN)),
            "an unordered compare keeps, exactly as `test ah,0x41; je` does"
        );
    }

    /// Two points 50 yd apart at the map's edge: six f32 roundings land exactly on 2500, one
    /// lands an ulp above. The literals round-trip to the intended f32s.
    #[test]
    fn a_six_rounding_kernel_lands_on_the_wrong_side_of_the_branch() {
        let a = Vec3::new(-6157.2476, 16978.16, -14441.062);
        let b = Vec3::new(-6180.6636, 16935.822, -14453.679);

        let naive = {
            let d = a - b;
            d.x * d.x + d.y * d.y + d.z * d.z
        };
        assert_eq!(
            naive, FOOTFALL_RADIUS_SQ,
            "six roundings land exactly on the threshold, which the strict compare would keep"
        );

        let ours = footfall_dist2(a, b);
        assert!(ours > naive, "one rounding lands an ulp above");
        assert!(
            footfall_culls(a, b),
            "so the reference culls this footfall, and so do we"
        );
    }

    /// The channels are disjoint: HumanMale's Walk keys one tag of each per footfall.
    #[test]
    fn the_sound_channel_is_fsd_alone() {
        assert!(is_footstep_sound(b"$FSD"));
        for t in [
            b"$FL0", b"$FR0", b"$RL2", b"$SL0", b"$SR0", b"$BR0", b"$WL1",
        ] {
            assert!(!is_footstep_sound(t), "{} is the visual channel", lossy(t));
            assert!(footfall_side(t).is_some(), "{} names a side", lossy(t));
        }
        assert_eq!(footfall_side(b"$FSD"), None);
    }

    #[test]
    fn footfall_side_reads_the_side_letter() {
        assert_eq!(footfall_side(b"$FL0"), Some(b'L'));
        assert_eq!(footfall_side(b"$FR0"), Some(b'R'));
        assert_eq!(footfall_side(b"$RL2"), Some(b'L'));
        assert_eq!(footfall_side(b"$BR0"), Some(b'R'));
        assert_eq!(footfall_side(b"$WR3"), Some(b'R'));
        assert_eq!(footfall_side(b"$SND"), None);
        assert_eq!(footfall_side(b"$CSL"), None);
    }

    fn lossy(t: &[u8; 4]) -> String {
        String::from_utf8_lossy(t).into_owned()
    }

    fn track() -> (TrackMemory, Entity, AnimationNodeIndex, AnimationNodeIndex) {
        (
            TrackMemory::default(),
            Entity::from_raw_u32(1).expect("valid entity id"),
            AnimationNodeIndex::new(7),  // ShuffleLeft, say
            AnimationNodeIndex::new(11), // Stand
        )
    }

    #[test]
    fn an_arm_frame_is_silent_and_the_next_frame_opens_the_head() {
        let (mut last, unit, shuffle, _) = track();
        assert_eq!(
            advance_track(&mut last, unit, shuffle, 0.0),
            None,
            "the arm frame itself scans nothing"
        );
        assert_eq!(
            advance_track(&mut last, unit, shuffle, 0.016),
            Some(-1.0),
            "the frame after the arm opens the head window [0, cur]"
        );
        assert_eq!(
            advance_track(&mut last, unit, shuffle, 0.032),
            Some(0.016),
            "…and steady frames scan (prev, cur] as before"
        );
    }

    /// Shuffle and Stand alternating every frame, as under a stuttering mouse-turn.
    #[test]
    fn a_clip_armed_for_one_frame_never_fires() {
        let (mut last, unit, shuffle, stand) = track();
        for _ in 0..8 {
            assert_eq!(advance_track(&mut last, unit, shuffle, 0.0), None);
            assert_eq!(advance_track(&mut last, unit, stand, 0.0), None);
        }
    }

    /// The corpse settle's `seek_to(duration)`: a body streamed in dead replays no collapse keys.
    #[test]
    fn an_arm_deep_in_the_timeline_never_opens_the_head() {
        let (mut last, unit, death, _) = track();
        assert_eq!(advance_track(&mut last, unit, death, 2.0), None);
        assert_eq!(
            advance_track(&mut last, unit, death, 2.0),
            Some(2.0),
            "the window opens at the settle stamp — (2.0, 2.0] is empty"
        );
    }

    /// A held turn lays a print pair on every 0.5 s wrap, as the reference's walker does.
    #[test]
    fn a_loop_wrap_refires_the_head_keys() {
        let clip = shuffle_clip();
        assert_eq!(fired(&clip, -1.0, 0.016), vec![*b"$SL0", *b"$SR0"], "head");
        assert!(
            fired(&clip, 0.016, 0.4).is_empty(),
            "mid-lap: nothing keyed"
        );
        assert_eq!(
            fired(&clip, 0.48, 0.01),
            vec![*b"$SL0", *b"$SR0"],
            "the wrap fires the tail (empty here) then the head — both feet again"
        );
    }

    /// HumanMale ShuffleLeft (`benilla-extract m2events`): 0.5 s, looping, `$SL0` and `$SR0` at 0.
    fn shuffle_clip() -> AnimClip {
        AnimClip {
            anim_id: 11,
            seq_index: 38,
            node: AnimationNodeIndex::new(7),
            looping: true,
            duration: 0.5,
            move_speed: 0.0,
            blend_time: 0.15,
            bounds_center: bevy::prelude::Vec3::ZERO,
            bounds_radius: 0.0,
            bounds_min: bevy::prelude::Vec3::ZERO,
            bounds_max: bevy::prelude::Vec3::ZERO,
            events: vec![
                benilla_assets::ClipEvent {
                    time: 0.0,
                    ident: *b"$SL0",
                    data: 0,
                    bone: 0,
                    offset: bevy::prelude::Vec3::ZERO,
                    point: bevy::prelude::Vec3::ZERO,
                },
                benilla_assets::ClipEvent {
                    time: 0.0,
                    ident: *b"$SR0",
                    data: 0,
                    bone: 0,
                    offset: bevy::prelude::Vec3::ZERO,
                    point: bevy::prelude::Vec3::ZERO,
                },
            ]
            .into(),
            arm_nodes: None,
            upper_node: None,
            frequency: 0,
            replay: (0, 0),
            poses_bones: true,
        }
    }

    /// The idents [`scan_events`] fires over one `(prev, cur]` window, in order.
    fn fired(clip: &AnimClip, prev: f32, cur: f32) -> Vec<[u8; 4]> {
        use bevy::ecs::system::RunSystemOnce;
        #[derive(bevy::prelude::Resource)]
        struct Window(AnimClip, f32, f32);
        let mut world = bevy::prelude::World::new();
        world.init_resource::<bevy::ecs::message::Messages<AnimSoundEvent>>();
        world.insert_resource(Window(clip.clone(), prev, cur));
        world
            .run_system_once(
                |win: bevy::prelude::Res<Window>, mut out: MessageWriter<_>| {
                    let unit = Entity::from_raw_u32(1).expect("valid entity id");
                    // Identity placement, no rig: the fired point is the key's own, zero here.
                    let world = GlobalTransform::IDENTITY;
                    let frame = EventFrame {
                        world: &world,
                        rig: None,
                    };
                    scan_events(&win.0, unit, win.1, win.2, &frame, &mut out);
                },
            )
            .expect("run_system_once");
        let mut msgs = world.resource_mut::<bevy::ecs::message::Messages<AnimSoundEvent>>();
        msgs.drain().map(|m| m.ident).collect()
    }

    #[test]
    fn a_rigless_models_key_fires_at_its_placed_and_scaled_point() {
        use bevy::ecs::system::RunSystemOnce;
        // A key 10 yd out along +x in model space, on a bone the model never animates.
        let mut clip = shuffle_clip();
        clip.events = vec![benilla_assets::ClipEvent {
            time: 0.5,
            ident: *b"$DSL",
            data: 1,
            bone: 3,
            offset: Vec3::new(1.0, 2.0, 3.0), // ignored: there is no rig to compose
            point: Vec3::new(10.0, 0.0, 0.0),
        }]
        .into();
        // Placed 100 yd north, turned a quarter turn, and at half scale.
        let placement = GlobalTransform::from(
            Transform::from_xyz(0.0, 0.0, 100.0)
                .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2))
                .with_scale(Vec3::splat(0.5)),
        );

        #[derive(bevy::prelude::Resource)]
        struct Placed(AnimClip, GlobalTransform);
        let mut world = bevy::prelude::World::new();
        world.init_resource::<bevy::ecs::message::Messages<AnimSoundEvent>>();
        world.insert_resource(Placed(clip, placement));
        world
            .run_system_once(|p: bevy::prelude::Res<Placed>, mut out: MessageWriter<_>| {
                let e = Entity::from_raw_u32(1).expect("valid entity id");
                let frame = EventFrame {
                    world: &p.1,
                    rig: None,
                };
                scan_events(&p.0, e, 0.0, 1.0, &frame, &mut out);
            })
            .expect("run_system_once");
        let mut msgs = world.resource_mut::<bevy::ecs::message::Messages<AnimSoundEvent>>();
        let fired: Vec<_> = msgs.drain().collect();
        assert_eq!(fired.len(), 1, "one key in the window");
        let at = fired[0]
            .pos
            .expect("a rig-less model still resolves a point");
        // Quarter turn about y takes +x to −z, halved by the scale, then translated.
        let want = placement.transform_point(Vec3::new(10.0, 0.0, 0.0));
        assert!(
            at.distance(want) < 1e-4,
            "fired at {at:?}, want {want:?} — the placement is not being applied"
        );
        assert!(
            at.distance(placement.translation()) > 1.0,
            "the point collapsed onto the model origin, which is the bug this exists to catch"
        );
    }
}
