//! Facing: the wire's `MonsterMoveFacing` resolution ([`resolve_facing`]) and the unit
//! **display-facing smoother** ([`drive_display_facing`]) — the client's `0x600cd0` goal chain and
//! its `+0xc98` box filter — with the [`FacingStep`] latch the anim layer's turn-shuffle reads.
//!
//! **A unit has two facings and only one of them is the wire's**.
//! The raw movement facing `CMovement+0x1c` is what the server put there; the *rendered character
//! body root* is the smoothed display facing `CGUnit+0xc94`, and the client turns that one
//! client-side, every frame, toward a goal it picks from an ordered chain. Two of that chain's goals
//! are local decisions the server never sends: a stationary unit squares up on its
//! `UNIT_FIELD_TARGET`, and **an NPC turns to face you for as long as its interaction window is
//! open** (bug B110).
//!
//! benilla collapses the two facings into the unit's `Transform.rotation`, which is therefore the
//! *display* facing; [`DisplayFacing::wire`] keeps the raw one alongside it, because the goal chain
//! falls back to the raw facing — and that fallback is what swings an NPC **back** to its authored
//! heading when you close its window. In the client the raw facing separately places the
//! collision/cull node, so a turning NPC's collision does not move; ours does. That asymmetry is a
//! named approximation, not fidelity: a unit's collision footprint is a capsule and its cull volume
//! a sphere, so rotating them is very nearly a no-op.

use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::{EntityKind, MonsterMoveFacing};
use bevy::prelude::*;

use super::super::{ActiveMover, GuidIndex, NetEntity, ObjectStore, SelfPlayer};
use super::{yaw_of, RemoteMotion, Spline};

/// The turn-shuffle latch's **sign dead-band** — the client's `[0x80c5c8]` = +1e-5 /
/// `[0x80c5c4]` = −1e-5 (`0x60843b`–`0x608473`), tested on the yaw **this pump actually applied**.
/// Above it the frame latches `+0xd58` bit `0x800` (→ ShuffleLeft 11), below its negative bit
/// `0x1000` (→ ShuffleRight 12), and between the two nothing latches: it is a *did the body move
/// at all* test, not a "close enough" one. Shared with the remote facing interp
/// ([`super::remote`]) — the same latch drives a standing remote's mouse-turn shuffle.
///
/// **This replaces `FACING_SETTLED = 0.05` (~3°), which was wrong twice over**.
/// 0123 recorded `0x80c5c4`/`0x80c5c8` as "the client's small/large-delta thresholds" and left an
/// "eyeballable stand-in" in their place; re-read at the bytes they are ±1e-5, a symmetric pair
/// about zero, and the two bits they gate are *left/right*, not small/large. Sixty times too wide,
/// and applied to the wrong quantity, the stand-in dropped the latch about half-way through every
/// turn — which is most of why a vendor turned to face you with frozen feet.
pub(super) const TURN_LATCH_BAND: f32 = 1.0e-5;

/// The filter's **dead-band** (`[0x8029d0]` = 0.01 rad ≈ 0.57°), tested on the *unfolded* goal−current
/// delta and **inclusive** (`fcomp` + `jp` at `600eb5`–`600ec0`). Inside it the goal is taken
/// verbatim and the history ring is reset, so a slowly-turning player is tracked with zero lag.
const DEAD_BAND: f32 = 0.01;

/// The box filter's average weight (`[0x8029b0]`) — a plain mean over the 4-sample ring.
const RING_AVG: f32 = 0.25;

/// The fraction of the (clamped) averaged delta applied per pump (`[0x7ffa24]`). With the
/// overshoot clamp binding on every approach pass this makes the steady behaviour a **per-frame
/// halving** of the error: ~8–9 frames from π down to [`DEAD_BAND`].
const STEP_FRACTION: f32 = 0.5;

/// `UNIT_FIELD_FLAGS & 0x40000` — `UNIT_FLAG_STUNNED`, goal-chain row 4.
const UNIT_FLAG_STUNNED: u32 = 0x0004_0000;

/// The `Emotes.dbc` `EmoteFlags` bit that **permits** the target/interaction facing while a looping
/// state emote is active (`600d95`/`600d98 test ch,0x20`). A valid emote-state record *without* this
/// bit suppresses the whole chain and pins the unit to its raw facing; the bit's *name* is
/// inferred — the byte fact is the condition tested.
const EMOTE_PERMITS_FACING: u32 = 0x2000;

/// A stationary unit's display-facing state — the client's `CGUnit+0xc98` goal plus its
/// `+0xc9c..+0xca8` delta-history ring. Inserted the first frame the unit is governed by
/// [`drive_display_facing`] and removed the moment it stops being (a path, a relay stream, a fresh
/// wire pose), so it is only ever alive while the client-local turn actually owns the body.
#[derive(Component)]
pub(crate) struct DisplayFacing {
    /// The **raw** (server-authoritative) yaw — the client's `CMovement+0x1c`. Seeded from the
    /// transform the wire left behind, and the goal every chain row but the two turn rows returns
    /// to. Nothing client-local ever writes it, which is exactly why the NPC swings back.
    wire: f32,
    /// The 4-sample delta history. A zero head means "start fresh" — the ring-FILL leg.
    hist: [f32; 4],
}

/// A stationary unit's display facing **moved this frame**: the signed yaw the pump applied (WoW
/// yaw, positive = counterclockwise = turning left). The anim layer folds it into the unit's turn
/// view — the client's facing-delta shuffle latch (`0x607ed0` bits `0x800`/`0x1000`;
/// decision 0123) — so a squaring-up creature foot-shuffles instead of
/// pivoting frozen, and each shuffle's return to Stand re-rolls the idle variation.
///
/// **The applied step, not the remaining gap**: the client's latch reads
/// `0x607ed0`'s accumulator — the increment it writes to `+0xc94` at `608239` — against
/// [`TURN_LATCH_BAND`]. The two differ at the tail of a per-frame-halving ease, which is exactly
/// where a shuffle is still meant to be playing. Removed the frame the body stops moving.
#[derive(Component)]
pub(crate) struct FacingStep(pub(crate) f32);

/// Which row of the goal chain a unit's facing goal came from — trace-only ([`trace_face`]), so a
/// "why is this NPC pointing there" question is answered from numbers instead of by eye.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GoalSource {
    /// Every row that pins the unit to its raw facing: moving, posed, emote-suppressed, stunned,
    /// or simply nothing to look at.
    Raw,
    /// Row 7 — the bearing to `UNIT_FIELD_TARGET`.
    Target,
    /// Row 9 — the bearing to the local player, because this unit is the interaction NPC.
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

/// Resolve a [`MonsterMoveFacing`] to a WoW orientation (radians) for a unit at `unit_pos` (raw WoW).
/// `Angle` is the orientation verbatim; `Spot`/`Target` are the horizontal bearing from the unit to
/// the point / the target unit's position (`atan2(dy, dx)`, the WoW convention that maps straight to
/// [`Quat::from_rotation_y`]). `target_pos` looks up another unit's raw-WoW position for `Target`.
/// `None` when there is nothing to face — [`MonsterMoveFacing::None`], an unknown/unstreamed target,
/// or a point coincident with the unit (a degenerate bearing that would spin it to 0).
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

/// The horizontal bearing from `from` to `to` (raw WoW positions), or `None` when the two are
/// essentially coincident — a degenerate `atan2` that would snap the unit to 0.
fn bearing(from: [f32; 3], to: [f32; 3]) -> Option<f32> {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    (dx * dx + dy * dy >= 1.0e-4).then(|| dy.atan2(dx))
}

/// Row 3 of the goal chain — the **emote-state gate** (`600d6c`–`600d9d`). A unit in a looping state
/// emote (`UNIT_NPC_EMOTESTATE`) is pinned to its raw facing *unless* the emote's `Emotes.dbc`
/// `EmoteFlags` carries [`EMOTE_PERMITS_FACING`]. An absent or unknown emote id suppresses nothing —
/// the client's `bl = 0` legs.
fn emote_suppresses_facing(emote_state: u32, flags: impl FnOnce(u32) -> Option<u32>) -> bool {
    if emote_state == 0 {
        return false;
    }
    match flags(emote_state) {
        Some(f) => f & EMOTE_PERMITS_FACING == 0,
        None => false, // no such record — the client's `rec == 0` leg
    }
}

/// One pump of the client's `+0xc98` box filter (`0x600cd0`, `600ea8`–`600fd5`), byte-for-byte.
/// Returns the new display yaw given the previous one, the goal and the unit's history ring.
///
/// **There is no `dt` in this filter** — it is per-*pump*, not per-second. The overshoot clamp binds
/// on every approach pass, so the steady behaviour is `delta_{n+1} = 0.5 · delta_n`. That is a
/// deliberate departure from a frame-rate-independent ease: it is the law the client runs, and at
/// any plausible frame rate it settles inside a sixth of a second either way.
fn filter_step(cur: f32, goal: f32, hist: &mut [f32; 4]) -> f32 {
    use std::f32::consts::TAU;
    // The dead-band is tested on the UNFOLDED delta and is inclusive (`600eb5`/`600ec0`).
    let raw_delta = goal - cur;
    if raw_delta.abs() <= DEAD_BAND {
        *hist = [0.0; 4];
        return goal.rem_euclid(TAU);
    }
    let delta = crate::creature_anim::wrap_pi(raw_delta);
    // A reversal empties the ring, so a turn that changes direction does not average against the
    // deltas it is undoing (`600f12`–`600f3b`).
    if delta * hist[0] < 0.0 {
        hist[0] = 0.0;
    }
    let step = if hist[0] == 0.0 {
        *hist = [delta; 4]; // the ring FILL leg — no averaging on this pass (`600f50`–`600f6a`)
        delta
    } else {
        hist.copy_within(0..3, 1); // `600f76 memmove(+0xca0, +0xc9c, 12)`
        hist[0] = delta;
        let avg = hist.iter().sum::<f32>() * RING_AVG;
        // Never overshoot the goal (`600f9e`–`600fc9`).
        if avg.abs() > delta.abs() {
            delta
        } else {
            avg
        }
    };
    (cur + step * STEP_FRACTION).rem_euclid(TAU)
}

/// Turn every **stationary, client-governed** unit toward the goal the client's `0x600cd0` chain
/// picks for it, through the one smoother that chain feeds ([`filter_step`]).
///
/// The chain, in the client's exact branch order — the ordering *is* the behaviour:
///
/// | row | test | goal |
/// |---|---|---|
/// | 0 | the body we steer | *(excluded by [`ActiveMover`])* |
/// | 1 | moving | raw — *(excluded by [`Spline`]; a patrolling NPC does not re-face)* |
/// | 2 | `standState != 0` — sitting, kneeling, dead | raw |
/// | 3 | a state emote whose `EmoteFlags` lacks [`EMOTE_PERMITS_FACING`] | raw |
/// | 4 | [`UNIT_FLAG_STUNNED`] | raw |
/// | 5 | the combat scripted override `+0xc58 & 1` → `+0xcac` | *(not modelled — see below)* |
/// | 6 | a remote player character | raw — *(excluded by [`RemoteMotion`])* |
/// | 7 | `UNIT_FIELD_TARGET` resolves | bearing to that unit |
/// | 9 | **this unit is the interaction NPC** | **bearing to the local player** |
/// | — | none of the above | raw |
///
/// Row 7 sitting above row 9 is why a vendor that is *fighting* keeps facing its attacker rather
/// than you. Row 5 is a combat-side facing override with a single writer in the client
/// (`0x624f10`, `UnitCombat_C.cpp`) and no benilla equivalent yet; it is a separate goal source, not
/// part of this one, and its absence only means we never pin a unit the client would have pinned.
///
/// **No range, line-of-sight, faction or is-alive test appears anywhere on this path** — the
/// `CanInteract` family is ruled out at the client's call graph. The only leash is the window's own
/// range close, which [`crate::ui_session::close_npc_session_out_of_range`] already owns.
///
/// `pub(crate)` rather than `pub(in crate::net)` for one reason: the shuffle latch's *consumer*
/// lives in `creature_anim`, and the only test that can catch the latch being produced but never
/// read has to run both systems in one app (`creature_anim::driver::tests`).
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
    // A remote player's latch belongs to the facing interp in [`super::remote`] — the cleanup here
    // must not sweep it (the two systems would fight over the component every frame).
    latched: Query<Entity, (With<FacingStep>, Without<RemoteMotion>)>,
    // A unit that has started walking or become a relay mover is no longer governed: its transform
    // belongs to the sampler / extrapolator, and its state must go so the next seed reads the pose
    // they left rather than a stale pre-walk heading.
    ungoverned: Query<Entity, (With<DisplayFacing>, Or<(With<Spline>, With<RemoteMotion>)>)>,
) {
    let self_pos = self_q
        .iter()
        .next()
        .and_then(|e| transforms.get(e).ok())
        .map(|t| bevy_to_wow(t.translation));
    let interact_npc = interact.and_then(|r| r.0);

    // Collect (unit, goal, source) from immutable position reads first, then apply — so the
    // target's transform and the unit's are never borrowed mutably at the same time.
    let mut goals: Vec<(Entity, Option<f32>, GoalSource)> = Vec::new();
    for (e, net, store) in &candidates {
        if net.kind != EntityKind::Unit {
            continue; // a GameObject sits at its authored facing; players are excluded above
        }
        let fields = &store.0;
        // Rows 2-4: the posture/state gates that pin a unit to its raw facing.
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
        // Row 7: the bearing to UNIT_FIELD_TARGET, when it resolves to something streamed.
        let target = fields
            .unit_target()
            .and_then(|g| index.0.get(&g).copied())
            .and_then(|te| transforms.get(te).ok())
            .and_then(|t| bearing(unit_pos, bevy_to_wow(t.translation)));
        if let Some(goal) = target {
            goals.push((e, Some(goal), GoalSource::Target));
            continue;
        }
        // Row 9: the interaction face-me. `InteractNpc` is the benilla-side twin of the client's
        // one global `[0xb4e2d0]` — whichever NPC window is open, and nothing when none is.
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
            // First frame governed: the transform still holds exactly what the wire (a pose, a
            // finished path) left there, so it *is* the raw facing. Seed and let the next pump
            // start from a clean ring — the client's `0x601020` shape.
            commands.entity(e).insert(DisplayFacing {
                wire: cur,
                hist: [0.0; 4],
            });
            continue;
        };
        // Every row but the two turn rows returns the unit to its raw facing — the fallback is a
        // *goal*, not a freeze, which is what swings an NPC back when you close its window.
        let goal = goal.unwrap_or(state.wire);
        let new = filter_step(cur, goal, &mut state.hist);
        // Write-on-change only (1473): a settled unit's `new` is bitwise `goal.rem_euclid(TAU)`
        // every frame (the dead-band leg of `filter_step` returns the goal verbatim, never a
        // re-derivation from `cur`), so this compare goes quiet one frame after settling. The
        // unconditional write it replaces marked every standing NPC's `Transform` changed every
        // frame — re-propagating its whole subtree and defeating each downstream `Changed` gate
        // for the entire idle crowd (1445's swim gate, the water-side classifier's moved sweep,
        // mesh re-extract) — the same defect `face_billboards` fixed for cards.
        let rot = Quat::from_rotation_y(new);
        if tf.rotation != rot {
            tf.rotation = rot;
        }
        // The turn-shuffle latch (corrected by 1650): the client tests the yaw
        // this pump APPLIED (`0x607ed0`'s `param_2`, the increment written to `+0xc94`) against
        // the symmetric ±[`TURN_LATCH_BAND`] sign band — not the gap still to cover, and not a
        // "close enough" threshold. Folded because a pump may cross the wrap.
        let step = crate::creature_anim::wrap_pi(new - cur);
        if step.abs() > TURN_LATCH_BAND {
            commands.entity(e).insert(FacingStep(step));
            stepping.insert(e);
        }
        trace_face(e, source, goal, cur, new);
    }
    // Drop stale latches: units that settled, lost their goal, or started moving this frame.
    for e in &latched {
        if !stepping.contains(&e) {
            commands.entity(e).remove::<FacingStep>();
        }
    }
    for e in &ungoverned {
        commands.entity(e).remove::<DisplayFacing>();
    }
}

/// The facing instrument (`WOW_MOVE_TRACE=<path> WOW_MOVE_TRACE_TAGS=face`) — one line per governed
/// unit per frame that actually turns, naming which goal-chain row won. "Why is this NPC pointing
/// there" is a numbers question; this is the number.
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

    /// The dead-band is INCLUSIVE and snaps the goal verbatim, resetting the ring — so a player
    /// strolling around a vendor is tracked with no lag at all.
    #[test]
    fn inside_the_dead_band_the_goal_is_taken_verbatim() {
        let mut hist = [0.3; 4];
        let out = filter_step(1.0, 1.0 + DEAD_BAND, &mut hist);
        assert!((out - (1.0 + DEAD_BAND)).abs() < 1.0e-6, "snapped: {out}");
        assert_eq!(hist, [0.0; 4], "the ring resets inside the dead-band");
    }

    /// The first pump off an empty ring takes the FILL leg: no averaging, half the delta.
    #[test]
    fn the_first_pump_fills_the_ring_and_halves_the_error() {
        let mut hist = [0.0; 4];
        let out = filter_step(0.0, FRAC_PI_2, &mut hist);
        assert!((out - FRAC_PI_2 / 2.0).abs() < 1.0e-5, "half way: {out}");
        assert_eq!(hist, [FRAC_PI_2; 4], "the ring fills with the delta");
    }

    /// The steady behaviour is a per-frame HALVING — not a per-second rate — and π converges into
    /// the dead-band in ~8-9 pumps. This is the shape benilla had wrong before.
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

    /// A reversal empties the ring so the new direction is not averaged against the deltas it is
    /// undoing — the client's sign-flip reset.
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

    /// The turn takes the short way round the circle, never the long one.
    #[test]
    fn the_turn_takes_the_short_arc() {
        let mut hist = [0.0; 4];
        // Goal is 0.2 rad counterclockwise of `cur`, expressed the long way (cur near TAU).
        let out = filter_step(TAU - 0.1, 0.1, &mut hist);
        let moved = crate::creature_anim::wrap_pi(out - (TAU - 0.1));
        assert!(moved > 0.0 && moved < 0.2, "short way, part way: {moved}");
    }

    /// A state emote suppresses the whole chain only when its record LACKS the permitting bit; an
    /// absent id and an unknown record both suppress nothing (the client's `bl = 0` legs).
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

    /// Drive the real system in a minimal app: a vendor 5 yd north of us, an interaction window
    /// open on it, and nothing else. It must turn to face us, settle, and — the client's
    /// distinctive observable — **swing back** to its authored heading when the window closes.
    ///
    /// This is the test that would have caught the mechanism being built and never firing: it
    /// exercises the goal chain and the `InteractNpc` wiring, not just [`filter_step`].
    #[test]
    fn the_interaction_npc_turns_to_face_us_and_swings_back_on_close() {
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .init_resource::<crate::ui_session::InteractNpc>()
            .add_systems(Update, drive_display_facing);
        // Us at the origin; the vendor 5 yd along +x, authored facing +x (away from us, which is
        // exactly the reporter's screenshot).
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
        // The first pump only seeds the state (the transform still holds the wire's own pose).
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
        // No window open yet: the goal is the raw facing, so it stays put however long we run.
        for _ in 0..16 {
            app.update();
        }
        assert!(
            (yaw(&app) - 0.0).abs() < 1.0e-4,
            "no window, no turn: {}",
            yaw(&app)
        );

        // Open a window on it. We are due WEST of the vendor, so the bearing it must adopt is pi.
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

        // Close it. The goal reverts to the untouched raw facing and the same filter swings the
        // body back — the client never wrote the raw facing, so neither did we.
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

    /// The turn-shuffle latch's own law: [`FacingStep`] carries **the yaw this
    /// pump applied** — the client's `0x607ed0` accumulator, the increment it writes to `+0xc94` —
    /// tested against the symmetric ±[`TURN_LATCH_BAND`] sign band (`[0x80c5c8]`/`[0x80c5c4]`).
    ///
    /// The regression this fences is the pair of substitutions 0123 made: the *remaining* gap in
    /// place of the applied step, and an eyeballed ~3° in place of ±1e-5. Because the ease halves
    /// its error every pump, "3° still to go" arrives while the body is still visibly moving, so
    /// the latch released about half-way through every turn and the foot-shuffle never blended in.
    /// The middle assertion is that exact frame: the latch is still held while the gap left is
    /// under the old threshold.
    #[test]
    fn the_latch_carries_the_step_applied_not_the_gap_remaining() {
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .init_resource::<crate::ui_session::InteractNpc>()
            .add_systems(Update, drive_display_facing);
        // Us off the vendor's axis: the bearing is +2.601 rad, reached counterclockwise, so every
        // step is positive — the left turn, and ShuffleLeft (11) downstream.
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
                // Nothing moved this frame, so nothing may be latched.
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

    /// Row 7 sits above row 9: a vendor that is *fighting* keeps facing its attacker, not us.
    #[test]
    fn a_target_outranks_the_interaction_face_me() {
        use benilla_protocol::ObjectFields;
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .init_resource::<crate::ui_session::InteractNpc>()
            .add_systems(Update, drive_display_facing);
        app.world_mut()
            .spawn((SelfPlayer, ActiveMover, Transform::default()));
        // The vendor's victim is due NORTH of it (+y in WoW), i.e. a bearing of +pi/2.
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
        // UNIT_FIELD_TARGET (index 16) is a guid pair; give the victim guid 7.
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

    /// The rotation write is a dead-band, not a metronome: once a governed unit settles, its
    /// `Transform` stops being marked changed. This is the regression fence for 1473's defeat
    /// finding — an unconditional same-value write re-propagated every standing NPC's subtree
    /// every frame and silently un-gated each downstream `Changed` consumer (1445's swim gate,
    /// the water-side classifier's moved sweep, mesh re-extract).
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
        // Us OFF the vendor's axis (bearing ≈ 2.60 rad), deliberately: a goal at exactly ±π sits
        // on the yaw wrap, where the client's unfolded-delta dead-band (its own byte-verified
        // quirk) never snaps — the ease still goes bitwise-quiet there, but only via float
        // underflow, ~16 pumps later. The representative case snaps inside the band in ~9.
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
        // Open a window on it: the vendor legitimately turns, dirtying the transform — the
        // control that proves the spy sees real writes.
        app.world_mut()
            .resource_mut::<crate::ui_session::InteractNpc>()
            .0 = Some(npc);
        app.update(); // seeding frame (spawn itself also reads as Changed here)
        app.update();
        assert!(
            app.world().resource::<Dirty>().0 > 0,
            "a turning unit dirties its transform"
        );
        // The turn settles inside the dead-band in ~9 pumps; run past it, then check steady state.
        for _ in 0..16 {
            app.update();
        }
        assert_eq!(
            app.world().resource::<Dirty>().0,
            0,
            "a settled unit must stop dirtying its transform"
        );
    }

    /// A coincident pair yields no bearing — a degenerate `atan2` would spin the unit to 0.
    #[test]
    fn a_coincident_pair_has_no_bearing() {
        assert!(bearing([1.0, 2.0, 3.0], [1.0, 2.0, 9.0]).is_none());
        let b = bearing([0.0, 0.0, 0.0], [1.0, 1.0, 0.0]).expect("a real bearing");
        assert!((b - std::f32::consts::FRAC_PI_4).abs() < 1.0e-5, "{b}");
    }
}
