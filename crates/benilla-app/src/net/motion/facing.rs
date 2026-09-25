//! Facing: resolving the wire's `MonsterMoveFacing`, and the unit display-facing smoother (the
//! 1.12 client's `0x600cd0` goal chain and `+0xc98` box filter) with the turn-shuffle latch.
//!
//! A unit has a raw wire facing (`CMovement+0x1c`) and a display facing (`CGUnit+0xc94`) that the
//! client turns every frame toward a goal from an ordered chain; two goals are local and never
//! sent: squaring up on `UNIT_FIELD_TARGET`, and an NPC facing you while its window is open.
//! `Transform.rotation` is the display facing; [`DisplayFacing::wire`] keeps the raw one, the
//! chain's fallback goal, which swings an NPC back when its window closes.
//! Deviation: the reference places collision and culling by the raw facing, ours follow the
//! display facing, because a capsule and a sphere barely change under rotation.

use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::{EntityKind, MonsterMoveFacing};
use bevy::prelude::*;

use super::super::{ActiveMover, GuidIndex, NetEntity, ObjectStore, SelfPlayer};
use super::{yaw_of, RemoteMotion, Spline};

/// The turn-shuffle latch's sign band, `[0x80c5c8]` = +1e-5 and `[0x80c5c4]` = -1e-5
/// (`0x60843b`-`0x608473`), tested on the yaw the pump applied: above it latches bit `0x800`
/// (ShuffleLeft 11), below it `0x1000` (ShuffleRight 12). Shared with the remote facing interp.
pub(super) const TURN_LATCH_BAND: f32 = 1.0e-5;

/// `[0x8029d0]`, in radians, tested inclusively on the unfolded goal-minus-current delta
/// (`600eb5`-`600ec0`); inside it the goal is taken verbatim and the ring resets.
const DEAD_BAND: f32 = 0.01;

/// `[0x8029b0]`: a plain mean over the 4-sample ring.
const RING_AVG: f32 = 0.25;

/// `[0x7ffa24]`: with the overshoot clamp binding, the error halves every pump.
const STEP_FRACTION: f32 = 0.5;

/// `UNIT_FLAG_STUNNED`, goal-chain row 4.
const UNIT_FLAG_STUNNED: u32 = 0x0004_0000;

/// The `EmoteFlags` bit that permits the target and interaction facing during a state emote
/// (`600d98 test ch,0x20`); the name is ours, the tested bit is the reference's.
const EMOTE_PERMITS_FACING: u32 = 0x2000;

/// A stationary unit's display-facing state, the client's `CGUnit+0xc98` goal and
/// `+0xc9c..+0xca8` delta ring; present only while [`drive_display_facing`] governs the unit.
#[derive(Component)]
pub(crate) struct DisplayFacing {
    /// The raw wire yaw (`CMovement+0x1c`), never written client-side.
    wire: f32,
    /// A zero head means the next pump takes the ring-fill leg.
    hist: [f32; 4],
}

/// The signed yaw the facing pump applied this frame (positive turns left), the client's
/// `0x607ed0` shuffle latch input; removed the frame the body stops moving.
#[derive(Component)]
pub(crate) struct FacingStep(pub(crate) f32);

/// Which goal-chain row won, for the `face` trace.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GoalSource {
    Raw,
    /// Row 7, the bearing to `UNIT_FIELD_TARGET`.
    Target,
    /// Row 9, the bearing to the local player from the interaction NPC.
    Interact,
}

impl GoalSource {
    fn tag(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Target => "target",
            Self::Interact => "interact",
        }
    }
}

/// A [`MonsterMoveFacing`] as a WoW orientation for a unit at `unit_pos`: `Spot` and `Target`
/// are the horizontal `atan2(dy, dx)` bearing; `None` also for an unstreamed or coincident target.
pub(in crate::net) fn resolve_facing(
    facing: MonsterMoveFacing,
    unit_pos: [f32; 3],
    target_pos: impl FnOnce(u64) -> Option<[f32; 3]>,
) -> Option<f32> {
    let bear = |to: [f32; 3]| {
        let (dx, dy) = (to[0] - unit_pos[0], to[1] - unit_pos[1]);
        (dx * dx + dy * dy > 1e-6).then(|| dy.atan2(dx))
    };
    match facing {
        MonsterMoveFacing::None => None,
        MonsterMoveFacing::Angle(a) => Some(a),
        MonsterMoveFacing::Spot(spot) => bear(spot),
        MonsterMoveFacing::Target(guid) => target_pos(guid).and_then(bear),
    }
}

/// The horizontal bearing from `from` to `to`, `None` when they coincide.
fn bearing(from: [f32; 3], to: [f32; 3]) -> Option<f32> {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    (dx * dx + dy * dy >= 1.0e-4).then(|| dy.atan2(dx))
}

/// Goal-chain row 3 (`600d6c`-`600d9d`): a state emote (`UNIT_NPC_EMOTESTATE`) whose
/// `Emotes.dbc` flags lack [`EMOTE_PERMITS_FACING`] pins the unit to its raw facing; an absent
/// or unknown emote suppresses nothing.
fn emote_suppresses_facing(emote_state: u32, flags: impl FnOnce(u32) -> Option<u32>) -> bool {
    if emote_state == 0 {
        return false;
    }
    match flags(emote_state) {
        Some(f) => f & EMOTE_PERMITS_FACING == 0,
        None => false, // the client's `rec == 0` leg
    }
}

/// One pump of the client's `+0xc98` box filter (`0x600cd0`, `600ea8`-`600fd5`). It has no `dt`:
/// the reference halves the error per frame, not per second.
fn filter_step(cur: f32, goal: f32, hist: &mut [f32; 4]) -> f32 {
    use std::f32::consts::TAU;
    let raw_delta = goal - cur;
    if raw_delta.abs() <= DEAD_BAND {
        *hist = [0.0; 4];
        return goal.rem_euclid(TAU);
    }
    let delta = crate::creature_anim::wrap_pi(raw_delta);
    // A reversal empties the ring (`600f12`-`600f3b`).
    if delta * hist[0] < 0.0 {
        hist[0] = 0.0;
    }
    let step = if hist[0] == 0.0 {
        *hist = [delta; 4]; // the ring-fill leg, no averaging (`600f50`-`600f6a`)
        delta
    } else {
        hist.copy_within(0..3, 1); // `600f76 memmove(+0xca0, +0xc9c, 12)`
        hist[0] = delta;
        let avg = hist.iter().sum::<f32>() * RING_AVG;
        // Never overshoot the goal (`600f9e`-`600fc9`).
        if avg.abs() > delta.abs() {
            delta
        } else {
            avg
        }
    };
    (cur + step * STEP_FRACTION).rem_euclid(TAU)
}

/// Turns every stationary unit toward the goal the client's `0x600cd0` chain picks, in its
/// branch order (the order is the behaviour):
///
/// | row | test | goal |
/// |---|---|---|
/// | 0 | the body we steer | excluded by [`ActiveMover`] |
/// | 1 | moving | raw, excluded by [`Spline`] |
/// | 2 | `standState != 0` | raw |
/// | 3 | a state emote lacking [`EMOTE_PERMITS_FACING`] | raw |
/// | 4 | [`UNIT_FLAG_STUNNED`] | raw |
/// | 5 | the combat override `+0xc58 & 1` to `+0xcac` (writer `0x624f10`) | not built |
/// | 6 | a remote player | raw, excluded by [`RemoteMotion`] |
/// | 7 | `UNIT_FIELD_TARGET` resolves | bearing to that unit |
/// | 9 | this unit is the interaction NPC | bearing to the local player |
/// | - | none of the above | raw |
///
/// No range, line-of-sight, faction or alive test is on this path; the window's own range close
/// is the only leash.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(crate) fn drive_display_facing(
    mut commands: Commands,
    index: Res<GuidIndex>,
    // `Option<Res<_>>` throughout: a headless test mounts this system without the UI plugins.
    interact: Option<Res<crate::ui_session::InteractNpc>>,
    emotes: Option<Res<crate::sound::EmoteSounds>>,
    candidates: Query<
        (Entity, &NetEntity, &ObjectStore),
        (Without<Spline>, Without<RemoteMotion>, Without<ActiveMover>),
    >,
    self_q: Query<Entity, With<SelfPlayer>>,
    mut transforms: Query<&mut Transform>,
    mut facings: Query<&mut DisplayFacing>,
    // A remote player's latch belongs to the remote facing interp; sweeping it here would fight it.
    latched: Query<Entity, (With<FacingStep>, Without<RemoteMotion>)>,
    // A unit that starts moving drops its state, so the next seed reads the pose it stops at.
    ungoverned: Query<Entity, (With<DisplayFacing>, Or<(With<Spline>, With<RemoteMotion>)>)>,
) {
    let self_pos = self_q
        .iter()
        .next()
        .and_then(|e| transforms.get(e).ok())
        .map(|t| bevy_to_wow(t.translation));
    let interact_npc = interact.and_then(|r| r.0);

    // Collect goals first, then apply, so no transform is borrowed mutably while another is read.
    let mut goals: Vec<(Entity, Option<f32>, GoalSource)> = Vec::new();
    for (e, net, store) in &candidates {
        if net.kind != EntityKind::Unit {
            continue; // a GameObject sits at its authored facing
        }
        let fields = &store.0;
        // Rows 2-4.
        if fields.unit_stand_state() != 0
            || fields.unit_flags() & UNIT_FLAG_STUNNED != 0
            || emote_suppresses_facing(fields.unit_emote_state(), |id| {
                emotes.as_ref().and_then(|c| c.emote_flags(id))
            })
        {
            goals.push((e, None, GoalSource::Raw));
            continue;
        }
        let Ok(unit_t) = transforms.get(e) else {
            continue;
        };
        let unit_pos = bevy_to_wow(unit_t.translation);
        // Row 7.
        let target = fields
            .unit_target()
            .and_then(|g| index.0.get(&g).copied())
            .and_then(|te| transforms.get(te).ok())
            .and_then(|t| bearing(unit_pos, bevy_to_wow(t.translation)));
        if let Some(goal) = target {
            goals.push((e, Some(goal), GoalSource::Target));
            continue;
        }
        // Row 9: `InteractNpc` stands for the client's global `[0xb4e2d0]`, the open NPC window.
        let facing_me = (interact_npc == Some(e))
            .then_some(self_pos)
            .flatten()
            .and_then(|p| bearing(unit_pos, p));
        match facing_me {
            Some(goal) => goals.push((e, Some(goal), GoalSource::Interact)),
            None => goals.push((e, None, GoalSource::Raw)),
        }
    }

    let mut stepping: bevy::ecs::entity::EntityHashSet = default();
    for (e, goal, source) in goals {
        let Ok(mut tf) = transforms.get_mut(e) else {
            continue;
        };
        let cur = yaw_of(tf.rotation).rem_euclid(std::f32::consts::TAU);
        let Ok(mut state) = facings.get_mut(e) else {
            // First frame governed: the transform still holds the wire's facing, so seed from it
            // with a clean ring (`0x601020`).
            commands.entity(e).insert(DisplayFacing {
                wire: cur,
                hist: [0.0; 4],
            });
            continue;
        };
        // The raw facing is a goal, not a freeze: it swings an NPC back when its window closes.
        let goal = goal.unwrap_or(state.wire);
        let new = filter_step(cur, goal, &mut state.hist);
        // Write only on change: a settled unit's `new` is bitwise the goal, so a standing crowd
        // stops tripping every downstream `Changed<Transform>` gate.
        let rot = Quat::from_rotation_y(new);
        if tf.rotation != rot {
            tf.rotation = rot;
        }
        // The latch tests the yaw this pump applied (`0x607ed0`'s `param_2`, the increment
        // written to `+0xc94`), not the gap left; folded because a pump may cross the wrap.
        let step = crate::creature_anim::wrap_pi(new - cur);
        if step.abs() > TURN_LATCH_BAND {
            commands.entity(e).insert(FacingStep(step));
            stepping.insert(e);
        }
        trace_face(e, source, goal, cur, new);
    }
    for e in &latched {
        if !stepping.contains(&e) {
            commands.entity(e).remove::<FacingStep>();
        }
    }
    for e in &ungoverned {
        commands.entity(e).remove::<DisplayFacing>();
    }
}

/// The `face` trace tag: one line per turning unit per frame, naming the goal-chain row that won.
fn trace_face(e: Entity, source: GoalSource, goal: f32, cur: f32, new: f32) {
    if !benilla_assets::trace::enabled_for("face") || (new - cur).abs() < 1.0e-6 {
        return;
    }
    benilla_assets::trace::line(
        "face",
        &format!(
            "e={e} src={} goal={goal:.4} cur={cur:.4} new={new:.4} step={:.4}",
            source.tag(),
            new - cur
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_assets::coords::wow_to_bevy;
    use std::f32::consts::{FRAC_PI_2, PI, TAU};

    /// The dead-band is inclusive.
    #[test]
    fn inside_the_dead_band_the_goal_is_taken_verbatim() {
        let mut hist = [0.3; 4];
        let out = filter_step(1.0, 1.0 + DEAD_BAND, &mut hist);
        assert!((out - (1.0 + DEAD_BAND)).abs() < 1.0e-6, "snapped: {out}");
        assert_eq!(hist, [0.0; 4], "the ring resets inside the dead-band");
    }

    #[test]
    fn the_first_pump_fills_the_ring_and_halves_the_error() {
        let mut hist = [0.0; 4];
        let out = filter_step(0.0, FRAC_PI_2, &mut hist);
        assert!((out - FRAC_PI_2 / 2.0).abs() < 1.0e-5, "half way: {out}");
        assert_eq!(hist, [FRAC_PI_2; 4], "the ring fills with the delta");
    }

    #[test]
    fn the_error_halves_per_pump_and_pi_settles_in_nine() {
        let mut hist = [0.0; 4];
        let (goal, mut cur) = (PI, 0.0);
        let mut pumps = 0;
        while crate::creature_anim::wrap_pi(goal - cur).abs() > DEAD_BAND && pumps < 64 {
            let prev = crate::creature_anim::wrap_pi(goal - cur).abs();
            cur = filter_step(cur, goal, &mut hist);
            let now = crate::creature_anim::wrap_pi(goal - cur).abs();
            assert!(now < prev, "each pump closes the gap: {prev} -> {now}");
            pumps += 1;
        }
        assert!(
            (8..=9).contains(&pumps),
            "pi settles in 8-9 pumps, took {pumps}"
        );
    }

    #[test]
    fn a_reversal_resets_the_ring_head() {
        let mut hist = [0.4; 4];
        filter_step(0.0, -1.0, &mut hist);
        assert!(
            hist[0] < 0.0,
            "the head carries the new direction: {hist:?}"
        );
        assert_eq!(
            hist[1], hist[0],
            "and the ring refilled rather than averaged"
        );
    }

    #[test]
    fn the_turn_takes_the_short_arc() {
        let mut hist = [0.0; 4];
        // The goal is 0.2 rad counterclockwise of `cur`, across the wrap.
        let out = filter_step(TAU - 0.1, 0.1, &mut hist);
        let moved = crate::creature_anim::wrap_pi(out - (TAU - 0.1));
        assert!(moved > 0.0 && moved < 0.2, "short way, part way: {moved}");
    }

    #[test]
    fn the_emote_state_gate_matches_the_clients_bl() {
        assert!(!emote_suppresses_facing(0, |_| Some(0)), "no emote state");
        assert!(!emote_suppresses_facing(7, |_| None), "unknown record");
        assert!(
            emote_suppresses_facing(7, |_| Some(0)),
            "a valid record without the bit suppresses"
        );
        assert!(
            !emote_suppresses_facing(7, |_| Some(EMOTE_PERMITS_FACING)),
            "the bit permits the facing"
        );
    }

    #[test]
    fn the_interaction_npc_turns_to_face_us_and_swings_back_on_close() {
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .init_resource::<crate::ui_session::InteractNpc>()
            .add_systems(Update, drive_display_facing);
        // Us at the origin; the vendor 5 yd along +x, facing +x, away from us.
        let me = app
            .world_mut()
            .spawn((SelfPlayer, ActiveMover, Transform::default()))
            .id();
        let _ = me;
        let npc = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                ObjectStore::default(),
                Transform {
                    translation: wow_to_bevy([5.0, 0.0, 0.0]),
                    rotation: Quat::from_rotation_y(0.0),
                    ..default()
                },
            ))
            .id();

        let yaw = |app: &App| yaw_of(app.world().entity(npc).get::<Transform>().unwrap().rotation);
        app.update();
        assert!(
            (yaw(&app) - 0.0).abs() < 1.0e-4,
            "the seeding frame does not turn: {}",
            yaw(&app)
        );
        assert!(
            app.world().entity(npc).contains::<DisplayFacing>(),
            "the seeding frame inserts the smoother state"
        );
        for _ in 0..16 {
            app.update();
        }
        assert!(
            (yaw(&app) - 0.0).abs() < 1.0e-4,
            "no window, no turn: {}",
            yaw(&app)
        );

        // We are due west of the vendor: a bearing of pi.
        app.world_mut()
            .resource_mut::<crate::ui_session::InteractNpc>()
            .0 = Some(npc);
        for _ in 0..16 {
            app.update();
        }
        let facing_us = yaw(&app);
        assert!(
            crate::creature_anim::wrap_pi(facing_us - PI).abs() <= DEAD_BAND,
            "the vendor faces us within the dead-band, got {facing_us}"
        );

        app.world_mut()
            .resource_mut::<crate::ui_session::InteractNpc>()
            .0 = None;
        for _ in 0..16 {
            app.update();
        }
        let back = yaw(&app);
        assert!(
            crate::creature_anim::wrap_pi(back - 0.0).abs() <= DEAD_BAND,
            "the vendor swings back to its authored heading, got {back}"
        );
    }

    /// The latch is held while under 0.05 rad of the turn remains, a gap a "close enough"
    /// threshold would release.
    #[test]
    fn the_latch_carries_the_step_applied_not_the_gap_remaining() {
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .init_resource::<crate::ui_session::InteractNpc>()
            .add_systems(Update, drive_display_facing);
        // A bearing of +2.601 rad, reached counterclockwise: every step is a left turn.
        app.world_mut().spawn((
            SelfPlayer,
            ActiveMover,
            Transform::from_translation(wow_to_bevy([0.0, 3.0, 0.0])),
        ));
        let npc = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                ObjectStore::default(),
                Transform {
                    translation: wow_to_bevy([5.0, 0.0, 0.0]),
                    rotation: Quat::from_rotation_y(0.0),
                    ..default()
                },
            ))
            .id();
        let yaw = |app: &App| yaw_of(app.world().entity(npc).get::<Transform>().unwrap().rotation);
        let step = |app: &App| app.world().entity(npc).get::<FacingStep>().map(|f| f.0);

        app.update(); // the seeding frame
        app.world_mut()
            .resource_mut::<crate::ui_session::InteractNpc>()
            .0 = Some(npc);

        let goal = 3.0f32.atan2(-5.0);
        let (mut latched_inside_the_old_band, mut pumps) = (false, 0);
        for _ in 0..24 {
            let before = yaw(&app);
            app.update();
            let after = yaw(&app);
            let applied = crate::creature_anim::wrap_pi(after - before);
            match step(&app) {
                Some(s) => {
                    pumps += 1;
                    assert!(
                        (s - applied).abs() < 1.0e-6,
                        "the latch carries the applied step: {s} vs {applied}"
                    );
                    assert!(s > 0.0, "a counterclockwise turn latches LEFT: {s}");
                    if crate::creature_anim::wrap_pi(goal - after).abs() < 0.05 {
                        latched_inside_the_old_band = true;
                    }
                }
                None => assert!(
                    applied.abs() <= TURN_LATCH_BAND,
                    "unlatched, but the body moved {applied}"
                ),
            }
        }
        assert!(
            (9..=11).contains(&pumps),
            "this bearing closes in ten pumps, got {pumps}"
        );
        assert!(
            latched_inside_the_old_band,
            "the latch outlives the old ~3° stand-in — that gap IS the missing half of the shuffle"
        );
        assert!(
            !app.world().entity(npc).contains::<FacingStep>(),
            "and drops once the body stops moving"
        );
    }

    /// Row 7 sits above row 9: a fighting vendor keeps facing its victim.
    #[test]
    fn a_target_outranks_the_interaction_face_me() {
        use benilla_protocol::ObjectFields;
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .init_resource::<crate::ui_session::InteractNpc>()
            .add_systems(Update, drive_display_facing);
        app.world_mut()
            .spawn((SelfPlayer, ActiveMover, Transform::default()));
        // The victim is due north of the vendor: a bearing of +pi/2.
        let victim = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                ObjectStore::default(),
                Transform::from_translation(wow_to_bevy([5.0, 5.0, 0.0])),
            ))
            .id();
        // UNIT_FIELD_TARGET is field 16, a guid pair; the victim is guid 7.
        let npc = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                ObjectStore(ObjectFields::from_pairs(&[(16, 7), (17, 0)])),
                Transform::from_translation(wow_to_bevy([5.0, 0.0, 0.0])),
            ))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(7, victim);
        app.world_mut()
            .resource_mut::<crate::ui_session::InteractNpc>()
            .0 = Some(npc);
        for _ in 0..24 {
            app.update();
        }
        let yaw = yaw_of(app.world().entity(npc).get::<Transform>().unwrap().rotation);
        assert!(
            crate::creature_anim::wrap_pi(yaw - FRAC_PI_2).abs() <= DEAD_BAND,
            "the vendor squares up on its victim (+pi/2), not on us (pi): {yaw}"
        );
    }

    #[test]
    fn a_settled_unit_stops_dirtying_its_transform() {
        #[derive(Resource, Default)]
        struct Dirty(usize);
        fn spy(q: Query<Entity, (Changed<Transform>, With<NetEntity>)>, mut out: ResMut<Dirty>) {
            out.0 = q.iter().count();
        }
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .init_resource::<crate::ui_session::InteractNpc>()
            .init_resource::<Dirty>()
            .add_systems(Update, (drive_display_facing, spy).chain());
        // Off the vendor's axis: a goal at exactly pi sits on the wrap, where the unfolded-delta
        // dead-band never snaps and the ease only goes quiet by underflow, ~16 pumps later.
        app.world_mut().spawn((
            SelfPlayer,
            ActiveMover,
            Transform::from_translation(wow_to_bevy([0.0, 3.0, 0.0])),
        ));
        let npc = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                ObjectStore::default(),
                Transform {
                    translation: wow_to_bevy([5.0, 0.0, 0.0]),
                    rotation: Quat::from_rotation_y(0.0),
                    ..default()
                },
            ))
            .id();
        // The control: a turning vendor dirties its transform.
        app.world_mut()
            .resource_mut::<crate::ui_session::InteractNpc>()
            .0 = Some(npc);
        app.update(); // seeding frame (spawn itself also reads as Changed here)
        app.update();
        assert!(
            app.world().resource::<Dirty>().0 > 0,
            "a turning unit dirties its transform"
        );
        // The turn settles in about nine pumps.
        for _ in 0..16 {
            app.update();
        }
        assert_eq!(
            app.world().resource::<Dirty>().0,
            0,
            "a settled unit must stop dirtying its transform"
        );
    }

    #[test]
    fn a_coincident_pair_has_no_bearing() {
        assert!(bearing([1.0, 2.0, 3.0], [1.0, 2.0, 9.0]).is_none());
        let b = bearing([0.0, 0.0, 0.0], [1.0, 1.0, 0.0]).expect("a real bearing");
        assert!((b - std::f32::consts::FRAC_PI_4).abs() < 1.0e-5, "{b}");
    }
}
