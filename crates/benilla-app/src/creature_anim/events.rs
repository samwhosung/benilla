//! The model event-keyframe scanner: [`fire_anim_events`] and its helpers, plus the
//! [`AnimSoundEvent`] message it emits — the animation timeline's outbound trigger surface
//! (decision 0070 slice 3). Kept in its own file as its own small concern, separate from the
//! driver ([`super::driver`]) that advances the clips it scans.

use benilla_assets::{AnimClip, ModelAnimations};
use bevy::animation::graph::AnimationNodeIndex;
use bevy::prelude::*;

use super::{resolved_id, AnimData, AnimDriver};
use crate::names::type_flags::MORE_AUDIBLE;

/// A model **event keyframe** crossed during playback this frame (the M2 `$xxx` tags — decision
/// 0070 slice 3): the animation timeline's outbound trigger surface. The sound subsystem routes
/// the audio tags (`$SND` kits, footsteps, `$CSS` swings, fidgets); future consumers (camera
/// shake `$SHK`, footprints) read the same stream.
#[derive(Message, Clone, Copy)]
pub(crate) struct AnimSoundEvent {
    pub(crate) entity: Entity,
    /// The forward-stored 4CC tag (`*b"$FL0"`, `*b"$SND"`, …).
    pub(crate) ident: [u8; 4],
    /// The tag payload (a SoundEntries id for `$SND`/`$DSL`/`$DSO`; 0 otherwise).
    pub(crate) data: u32,
    /// The `AnimationData.dbc` id of the **clip that fired the key**.
    ///
    /// This is the reference's `0x5fdb50` answer at the instant a handler runs: that helper reads
    /// the model's currently-playing animation id, and a key fires from a clip the model is
    /// playing, at that key's own time in it. Two dispatch arms branch on it — the `$BWR` weapon
    /// family fork (`0x5fcfb0` bow {46,105,109} vs `0x5fcfd0` rifle {49,106,110}, decision 2281) —
    /// and carrying it here is what lets them ask without re-deriving "what is this unit playing"
    /// from a player whose one-shot overlays outrank its base clip in weight.
    pub(crate) anim_id: u16,
    /// **Where the key fired** — the reference's own `placementMatrix · (boneMatrix[event.bone] ·
    /// event.position)`, resolved here at the fire and carried by value exactly as the M2 event
    /// kernel `0x719370` snapshots it into the deferred callback record that every dispatcher
    /// (`0x5ffbd0` units, `0x5f3e20` GameObjects, `0x6951e0` placed models) then reads (decision
    /// 1904).
    ///
    /// It is emphatically not the object's origin: corpus-wide 149 of 244 `$DSL` records sit off
    /// it, out to **67.6 yd** on `Maraudon_Waterfall01.m2`, and the six `$CSD` records every
    /// player model authors ride the head. `None` only when the model carries no rig *and* no
    /// world frame to place it in — the consumer then falls back to the object's own transform,
    /// which is what all of them used to do unconditionally.
    pub(crate) pos: Option<Vec3>,
}

/// The frame a scanner resolves a fired key's world point in — the two exact cases, named.
///
/// A **rigged** model composes the record's bone-local offset through that bone's live joint
/// global, which is the kernel's `boneMatrix[bone] · position` and then the placement. A model
/// with **no rig** has no bone matrices at all, and with no keys every bone composes to the
/// identity — so `placement · position` is not an approximation of the kernel's quantity, it *is*
/// it. That second case is the whole placed-doodad population: 0 of 244 `$DSL` records ride a bone
/// any sequence keys (`benilla-extract eventmarkerscan`).
pub(crate) struct EventFrame<'a> {
    /// The model's own world frame — the reference's placement matrix.
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

/// A footfall is **two independent channels**, and a tag belongs to exactly one of them — the
/// client's event dispatcher `0x5ffbd0` routes them to two different handlers:
///
/// - **`$FSD` → `0x623390`: the footstep SOUND**, and nothing else — it never reaches the decal
///   path ([`is_footstep_sound`]).
/// - **the per-foot side tags → `0x5fbf70`: the VISUAL footfall** — the footprint decal and the
///   spray/splash particle, and no sound at all ([`footfall_side`]).
///
/// So a gait that authors both fires **one** sound per `$FSD` key, not one per key of either
/// family: HumanMale's Walk keys `$FR0 · $FSD · $FL0 · $FSD` over 1 s and the real client plays
/// **two** steps there, while its turn-in-place ShuffleLeft/Right key only `$SL0 $SR0` at
/// `t = 0.000` and are **silent** (decision 1080).
///
/// The **sound** channel: the dispatch tag `$FSD` alone.
pub(crate) fn is_footstep_sound(ident: &[u8; 4]) -> bool {
    ident == b"$FSD"
}

/// **The footfall handler's own radius**, `2500 yd²` (50 yd) — `[0x80c5b4]`, read at `0x5fc00a`
/// inside `0x5fbf70`. This gate stands *above* everything that handler goes on to spawn: the
/// footprint decal ([`crate::footprints`]), the footstep camera shake
/// ([`crate::camera_shake`]) and the 25 yd spray branch below it all live under it, so it is one
/// constant and not one per consumer — they held a copy each until they disagreed about its
/// origin (decision 1856).
///
/// **Measured from the CAMERA EYE, not the local player.** `FUN_004818f0()` returns
/// `[[0xb4b2bc]+0x65b8]` — the *active camera* — and its `+0x8/+0xc/+0x10` is the eye.
///
/// **Unconditional** — the local player's own feet are gated like anyone else's. The reference's
/// `GUID == local player` compare (`0x5fc042`–`0x5fc06b`) is the decal call's fifth argument, the
/// **ring-pool select**, not an early bypass.
const FOOTFALL_RADIUS_SQ: f32 = 2500.0;

/// **Is this footfall out of range?** The handler's distance gate, whole: the kernel, the
/// threshold and the compare, because all three are load-bearing and splitting them is what
/// let two consumers disagree.
///
/// The kernel rounds **once, at the end**. The reference is x87: it loads the two `C3Vector`s as f32, and keeps every difference, square
/// and sum in 80-bit extended, then `fstp`s the sum to a **single f32** (`0x5fbffe` store,
/// `0x5fc007` reload) before the compare. A plain `Vec3::distance_squared` rounds six times
/// instead of once and can land on the other side of the branch for a footfall sitting on the
/// 50 yd edge.
///
/// f64 reproduces it exactly and needs no x87: an `f32 − f32` widened first is exact, a 24×24→48
/// bit product is exact, and the sum of three ≤50-bit values is exact — which also makes the
/// reference's `(dz² + dy²) + dx²` summation order unobservable here, so this does not pretend to
/// map its axis names onto ours. Round once on the way out and the f32 handed to the compare is
/// bit-identical.
///
/// The compare is **strict** — bail iff `> FOOTFALL_RADIUS_SQ`, so exactly 50 yd passes, and so
/// does a NaN: `test ah,0x41; je` reads ZF and takes the unordered result as "keep", which Rust's
/// `>` reproduces for free. It lives here rather than at the call sites so the strictness and the
/// NaN edge are stated once instead of re-spelled per consumer.
pub(crate) fn footfall_culls(a: Vec3, b: Vec3) -> bool {
    footfall_dist2(a, b) > FOOTFALL_RADIUS_SQ
}

/// The gate's `dist²`, split out so the tests can see the number the compare is handed.
fn footfall_dist2(a: Vec3, b: Vec3) -> f32 {
    let d = |p: f32, q: f32| p as f64 - q as f64;
    let (dx, dy, dz) = (d(a.x, b.x), d(a.y, b.y), d(a.z, b.z));
    (dx * dx + dy * dy + dz * dz) as f32
}

/// The **visual** channel: the foot side a per-foot plant tag names (`$FL0` → `L`), or `None` for
/// every other tag — the trigger for footfall-driven *visuals* ([`crate::footprints`]). The ten
/// families the dispatcher tables, each with its `0/1/2/3` variants: `$FL/$FR` (front),
/// `$RL/$RR` (rear), `$SL/$SR` (shuffle), `$BL/$BR` (backwards), `$WL/$WR` — call sites
/// `0x5ffe32` (`push 1`, LEFT) and `0x5ffc82` (`push 0`, RIGHT).
pub(crate) fn footfall_side(ident: &[u8; 4]) -> Option<u8> {
    matches!(
        &ident[..3],
        b"$FL" | b"$FR" | b"$RL" | b"$RR" | b"$SL" | b"$SR" | b"$BL" | b"$BR" | b"$WL" | b"$WR"
    )
    .then_some(ident[2])
}

/// How far (seconds) into a just-armed clip the playhead may have been *at the arm* for
/// [`advance_track`] to open the next frame's window at the clip's head (`t = 0` keyframes
/// included). Comfortably above a frame (even a hitchy one), comfortably below the corpse
/// settle's `seek_to(duration)` (every Death clip is > 1 s).
const FRESH_CLIP_HEAD: f32 = 0.25;

/// One scanned track's memory: the graph node playing, the seek last seen on it, and whether that
/// seek is an **arm stamp** — the frame the clip was armed, which fires nothing (see
/// [`advance_track`]).
#[derive(Clone, Copy)]
pub(crate) struct TrackSeek {
    node: AnimationNodeIndex,
    seek: f32,
    armed: bool,
}

/// A scanner's per-entity track memory — the `Local` every [`advance_track`] caller owns.
pub(crate) type TrackMemory = bevy::ecs::entity::EntityHashMap<TrackSeek>;

/// Fire the event keyframes the current clip crossed since last frame. Runs after
/// [`super::driver::drive_animations`] so the clip/seek state is this frame's. Per unit we
/// remember the playing node and its seek ([`TrackSeek`]); a loop wrap fires the tail
/// `(prev, duration]` then the head `[0, cur]` — a `t = 0` key really does re-fire on every wrap,
/// in the reference too (the walker `0x719370`), which is why a held turn-in-place
/// lays a footprint pair twice a second there as well.
///
/// Arming is [`advance_track`]'s: an arm frame fires nothing, the frame after it opens the clip's
/// head window (so `t = 0` keyframes are real), and a clip that doesn't survive its arm frame
/// fires nothing at all — the reference's own walker rule, byte-cited there.
#[allow(clippy::type_complexity)] // one Bevy query tuple — the house convention
pub(super) fn fire_anim_events(
    units: Query<(
        Entity,
        &ModelAnimations,
        &AnimationPlayer,
        &AnimDriver,
        Has<benilla_world::rig_anim::AnimParked>,
        // The unit's descriptor — read for ONE field, `OBJECT_FIELD_ENTRY`, the key its cached
        // creature template hangs off (see the `MORE_AUDIBLE` read below).
        Option<&crate::net::ObjectStore>,
        &GlobalTransform,
        Option<&benilla_world::rig_anim::RigPose>,
    )>,
    // The joint roots the composed poses hang off — read, never spawned (decision 1355's pure
    // position read).
    globals: Query<&GlobalTransform>,
    mut last: Local<TrackMemory>,
    // The **masked overlay** track's own memory (decision 0087): a swing/emote routed to the
    // SpineLow overlay plays *beside* the base, so its events (a swing's `$CSS`, an emote's `$CSD`)
    // are scanned on their own node — the base scan above never sees them.
    mut last_overlay: Local<TrackMemory>,
    mut out: MessageWriter<AnimSoundEvent>,
    anim_data: Option<Res<AnimData>>,
    names: Res<crate::names::NameCache>,
) {
    let catalog = anim_data.as_deref().map(|d| &d.0);
    for (entity, anims, player, drv, parked, store, world, pose) in &units {
        // The election's TICK half (decision 1482): a parked unit's event tracks are not
        // scanned — the reference's pass-2 walk never inserts the model into the tick worklist
        // (`0x683dd0` walk 2 skips `0x710b90`) — unless its cached template carries
        // `MORE_AUDIBLE`, the `0x607da0` re-link arm that keeps an off-screen flagged
        // creature's combat audible. A missing template record reads NOT audible (the
        // reference's `0x623b70` null leg — fail closed; the record lands within a second of
        // streaming anyway). The track memories are dropped so a waking track re-ARMS — firing
        // nothing on the wake frame — instead of scanning the whole parked gap as one
        // crossing; the reference's re-admission is likewise a fresh record.
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
        // Base track: the **resolved** id (decision 0082) — the clip whose timeline is actually
        // advancing, which can differ from the requested `active_anim()` when this model falls back —
        // then the id's **playing variation** (decision 0114: a one-shot rolled one of the id's
        // variation clips, each its own node with its own event track). During a same-id cross-fade
        // (swing variation A fading under fresh variation B) the newest play — the smallest seek —
        // is the track; the node-keyed memory then treats the switch as a clip change.
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
        // Masked overlay track: events fire from whichever track plays the clip. The overlay knows
        // its exact node — match it back to its clip (a variation's `upper_node`, decision 0114).
        // A freshly-started overlay fires its head window one frame after the arm via
        // [`advance_track`], so an emote's `t = 0` `$CSD` voice still rings; a swing's mid-clip
        // `$CSS` fires as normal.
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

/// Advance a per-track memory and return the `prev` seek to scan events from, or `None` to only
/// arm this frame. Keyed by the playing **graph node** (not the semantic id): two variations of
/// the same id are different timelines with different event tracks (decision 0114), and a node
/// switch is a clip change like any other.
///
/// **The arm frame fires nothing** — the reference's own rule, and the reason this is not simply
/// "fire the head window when you see a new clip" (decision 1273). The client's animation arm
/// `0x7121a0` bakes the block's window start `+0xa8 = now` (`0x712758`), so on that frame the
/// walker computes `prev == cur` and `0x719518 jae` abandons the block before reading a single
/// key. The window only opens on the **next** frame, and it opens at the arm stamp — local
/// `t = 0` — with the fire test `prevL <= t < curL` (`0x7196d5`–`0x7196d9`), so `t = 0` keyframes
/// are real (the emote voices carry `$CSD` at `0.000`) but only for a clip that is **still armed a
/// frame later**. A clip armed and abandoned inside one frame fires nothing at all, which is what
/// keeps a flickering Shuffle↔Stand churn silent in the reference — and is exactly what our
/// fire-on-the-arm-frame rule turned into a footprint carpet under a stuttering mouse-turn.
///
/// So: a clip change (and first sight of a unit — it may have streamed in mid-clip) records the
/// arm and returns `None`; the frame after an arm returns `-1.0`, opening the head window
/// `[0, cur]`. An arm that *starts* deep in its timeline (the corpse settle's `seek_to(duration)`,
/// past [`FRESH_CLIP_HEAD`]) opens at its own stamp instead, so a settled corpse never replays its
/// collapse's keys.
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

/// Fire the event keyframes `clip` crossed on `(prev, cur]`. A loop wrap (`cur < prev`) fires the
/// tail `(prev, duration]` of the last cycle then the head `[0, cur]` of the new one.
///
/// **Every fired key is traced under `aev`** (`WOW_MOVE_TRACE`, `WOW_MOVE_TRACE_TAGS=aev`). This is
/// the *asking* half of the sound instrument, and it was the half we did not have: the play log
/// (`RUST_LOG=benilla_app::sound=debug`) says what sounded, but a report of the shape "this
/// creature vocalises far too often" needs to separate *the tag fired too often* from *the tag
/// fired as authored and the gate above it is missing*. Those are different bugs with different
/// fixes — decision 1399 had to answer exactly that question for a pet owl and could only do it by
/// reading the M2 by hand. One line per key, on the same clock as the mover and wire traces, so a
/// vocal can be read against the clip that asked for it.
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
                            // How far the key fired from the model's own origin — the whole
                            // question this line was extended to answer (decision 1904). A
                            // non-zero `off` is the marker being honoured; all-zero across a run
                            // is the model-root fallback, which is what it used to be everywhere.
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

    /// The gate's edge behaviour, which is the whole reason the kernel is not
    /// `Vec3::distance_squared`: exactly 50 yd **passes** (the compare is strict), a NaN passes
    /// with it, and the sum is rounded to f32 exactly once.
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

    /// **Rounding once is not cosmetic** — this pair flips the branch.
    ///
    /// Two points 50 yd apart out at the map's edge, where the coordinates are large enough that
    /// f32 differences and squares stop being exact. Rounding six times (each difference, each
    /// square, each partial sum — what a plain f32 `distance_squared` does) lands on **exactly**
    /// `2500.0`, which the strict compare *keeps*; the reference's single rounding lands one ulp
    /// above, which it *culls*. Same two points, opposite answers, and ours has to be the second.
    ///
    /// The literals are the shortest decimals that round-trip to the intended f32s.
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

    /// The two channels are disjoint (decision 1080): `$FSD` is the whole sound channel, the
    /// per-foot side tags the whole visual one. HumanMale's Walk keys one of each family per
    /// footfall — routing both to sound is exactly the doubled step rate.
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

    /// The side letter a per-foot tag names; every other tag is `None`.
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

    /// **The arm frame scans nothing, and the frame after it opens the clip's head.** The
    /// reference's arm (`0x7121a0`) stamps the block's window start at `now`, so the walker's
    /// `prev == cur` abandons the block that frame (`0x719518`); the next frame's window opens at
    /// that stamp — local `t = 0`, lower-inclusive — which is what makes an emote's `t = 0.000`
    /// `$CSD` real without making it fire a frame early.
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

    /// **A clip armed and abandoned inside one frame fires nothing** — the report this rule was
    /// written for (decision 1273). A stuttering mouse-turn flickers the gait Shuffle↔Stand at
    /// input cadence, and HumanMale's ShuffleLeft keys `$SL0`+`$SR0` at `t = 0.000`: firing on the
    /// arm frame laid a footprint pair per flicker, where the reference lays none.
    #[test]
    fn a_clip_armed_for_one_frame_never_fires() {
        let (mut last, unit, shuffle, stand) = track();
        for _ in 0..8 {
            assert_eq!(advance_track(&mut last, unit, shuffle, 0.0), None);
            assert_eq!(advance_track(&mut last, unit, stand, 0.0), None);
        }
    }

    /// An arm that *starts* deep in its timeline — the corpse settle's `seek_to(duration)` —
    /// opens at its own stamp, never at the head: a body that streamed in dead must not replay
    /// the collapse's `$DTH` keys.
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

    /// The turn-in-place cadence, which this change deliberately leaves alone: HumanMale's
    /// ShuffleLeft (anim 11) is a **0.500 s loop** whose only keys are `$SL0` and `$SR0`, both at
    /// `t = 0.000`, and the reference re-fires a `t = 0` key on **every wrap** (the walker
    /// `0x719370`) — so a held turn lays a print pair twice a
    /// second in the real client too.
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

    /// HumanMale ShuffleLeft's real shape (`benilla-extract m2events`): 0.500 s, looping, keys
    /// `$SL0` and `$SR0` both at 0.000.
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
                    // Identity placement, no rig: the fired point is the key's own model-space
                    // one, which is what this test's zero-offset keys make `Vec3::ZERO`.
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

    /// **A rig-less model's event point is the placement times the key's own model-space point** —
    /// and that is not an approximation of the kernel's `placementMatrix · (boneMatrix[bone] ·
    /// position)`: with no keys every bone matrix composes to the identity, so the two are equal.
    ///
    /// This is the whole placed-doodad population, where the offsets are largest (149 of 244
    /// shipped `$DSL` records sit off their origin, out to 67.6 yd) and **none** rides a bone any
    /// sequence keys. A model *placed* somewhere and *scaled* must carry its marker with it, which
    /// is what the transform below checks and what a bare `translation()` read could not do.
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
            offset: Vec3::new(1.0, 2.0, 3.0), // ignored on this leg — there is no rig to compose
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
