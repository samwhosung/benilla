//! Terrain conform — **fidelity** (`0x7106c0`): the
//! reference tilts every model whose M2 authors `GlobalModelFlags & 3 ∈ {1,3}` — all 152 mounts
//! + quadrupeds pitch (flag 1), the 74 low-wide models (kodo/basilisk/crab/crocodile/spider)
//! pitch AND roll (flag 3) — wild or mounted alike: the gate is the model flag, not mountedness.
//! The [`ConformNode`] is the tilt carrier: an extra node a flagged model's root bones parent
//! under (spawned by [`super::attach`]), so one rotation write tilts the whole composite — a
//! mount's seat joint, rider included, hangs below it — while the unit root keeps its net-driven
//! yaw and the collider/selection footprint stay upright. `WOW_NO_MOUNT_TILT=1` disables (the
//! A/B lever; the name predates this creature generalization and is kept stable).

use avian3d::prelude::SpatialQuery;
use bevy::ecs::entity::{EntityHashMap, EntityHashSet};
use bevy::prelude::*;

/// Ray envelope around the unit's feet: start this far up, reach this far down past the feet.
const TILT_PROBE_UP: f32 = 2.0;
const TILT_PROBE_DOWN: f32 = 4.0;
/// The walkable cutoff on a sampled face normal — the producer's `[0x80e028]` ≈ cos 89°: a
/// steeper face isn't ground at all (zero qualifying contacts → the ref writes world-up).
const TILT_WALKABLE_Y: f32 = 0.0175;
/// The freeze band — the bridge's `[0x80c6a8]` = 0.3572 ≈ cos 69.07°: ground steeper than ~69°
/// FREEZES the smoothed up-vector (holds the last stance) instead of conforming.
const TILT_FREEZE_Y: f32 = 0.3572;
/// The up-vector smoothing decay — the bridge's `[0x80c6a0]`: per-frame factor `0.0018^dt`
/// (≈ `e^(−6.32·dt)`, τ ≈ 158 world) applied to the RESIDUAL toward the target normal.
const TILT_DECAY: f32 = 0.0018;

/// The tilt carrier a flagged model's root bones parent under — spawned by the attach path when
/// the built display's `terrain_tilt != 0`, despawned with the visual (it's a child of the
/// unit). [`conform_units`] writes its local rotation each frame: the ref's `0x7106c0` model
/// matrix stage isolated into one node.
#[derive(Component)]
pub(super) struct ConformNode {
    /// The streamed unit whose feet sample the ground and whose yaw frames the normal — for a
    /// mount child's node, the HOST unit (the mount body sits at the unit matrix).
    pub(super) unit: Entity,
    /// The model's `GlobalModelFlags & 3` dispatch mode (1 = pitch, 3 = pitch + roll).
    pub(super) mode: u8,
}

/// The per-frame target for the smoothed up-vector, from one sampled ground-face normal — the
/// producer + bridge gates composed (`0x637140` / `0x614cd0`): a miss or an unwalkable face
/// (steeper than ~89°) targets world-up; a face in the freeze band (69°..89°) holds the current
/// stance (`None`); a walkable face tracks its normal.
fn tilt_target(sampled: Option<Vec3>) -> Option<Vec3> {
    match sampled {
        Some(n) if n.y < TILT_WALKABLE_Y => Some(Vec3::Y),
        Some(n) if n.y < TILT_FREEZE_Y => None,
        Some(n) => Some(n),
        None => Some(Vec3::Y),
    }
}

/// The conform rotation for a smoothed up-vector in the unit's LOCAL frame (yaw already applied
/// by the parent) — the `0x7106c0` flag dispatch, byte-exact:
///
/// - **flag 1 (pitch only, `0x710769`)**: left is forced horizontal (`row1.z` a hard literal 0),
///   forward = left × up — which reduces in the local frame to a pure rotation about local X by
///   `atan2(n.z, n.y)` (the roll component of the normal is discarded).
/// - **flag 3 (pitch + roll, `0x710a80`)**: left = up × forward (NOT horizontal), forward =
///   left × up, and the model's up row is the normal **verbatim** — full conform.
/// - anything else: level (flag 2 is byte-verified inert; no shipped M2 authors it).
///
/// No clamp and no easing here — the ref has neither; all smoothing lives on the up-vector input.
fn conform_rotation(mode: u8, n_local: Vec3) -> Quat {
    match mode {
        1 => Quat::from_rotation_x(n_local.z.atan2(n_local.y)),
        3 => {
            let up = n_local.normalize_or_zero();
            // left = up × fwd0 (WoW row order, mapped to the Bevy local frame: fwd = −Z,
            // left = −X, up = +Y — right-handed).
            let left = up.cross(Vec3::NEG_Z);
            if up == Vec3::ZERO || left.length_squared() < 1e-6 {
                return Quat::IDENTITY; // degenerate normal (≈ along the facing axis)
            }
            let left = left.normalize();
            let fwd = left.cross(up).normalize();
            Quat::from_mat3(&Mat3::from_cols(-left, up, -fwd))
        }
        _ => Quat::IDENTITY,
    }
}

/// Conform every flagged model to the terrain under its unit. The pipeline
/// mirrors the ref's three stages: a ground normal per unit (theirs: the averaged walkable
/// collision-contact normal `CMovement+0x24`; ours: ONE ground-face normal from a down-ray
/// against the decal-receiver set — a named approximation that coincides on uniform slopes),
/// exponentially smoothed **on the up-vector** at `0.0018^dt` with the ~69° freeze band, then
/// the flag-picked basis written to the [`ConformNode`] — no angle clamp anywhere.
///
/// Two named divergences from the ref's every-unit-every-frame maintenance, both invisible by
/// construction: only units carrying a flagged visual are tracked (a flag-0 unit's vector is
/// never consumed), with the first sight seeding AT the slope — warm, so a mount-up or a fresh
/// spawn seats instantly like the reference; and a parked rig (off-frustum, the 0448 anim-LOD
/// gate) skips its ray and drops its vector, re-seeding warm on wake — the 0448 absolute-snap
/// philosophy applied to the stance, bounding the cost at one ray per flagged unit ON SCREEN.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(super) fn conform_units(
    time: Res<Time>,
    spatial: SpatialQuery,
    decals: benilla_world::decal::WorldDecal,
    units: Query<(&Transform, Has<benilla_world::rig_anim::AnimParked>), Without<ConformNode>>,
    mut nodes: Query<(&ConformNode, &mut Transform)>,
    mut ups: Local<EntityHashMap<Vec3>>,
    mut disabled: Local<Option<bool>>,
    // Once-a-second census at debug level — "is anything actually tilting, and how much",
    // answerable from a probe log (the blob-shadow census pattern).
    mut census_at: Local<f32>,
) {
    if *disabled.get_or_insert_with(|| std::env::var_os("WOW_NO_MOUNT_TILT").is_some()) {
        return;
    }
    let filter = benilla_world::collision::WorldCollision::body_filter();
    let decay = TILT_DECAY.powf(time.delta_secs());
    let mut seen = EntityHashSet::default();
    let mut max_pitch: Option<(Entity, f32)> = None;
    let mut count = 0usize;
    for (node, mut node_tf) in &mut nodes {
        let Ok((unit_tf, parked)) = units.get(node.unit) else {
            continue;
        };
        if parked {
            ups.remove(&node.unit);
            continue;
        }
        // Sample + smooth once per unit per frame (a unit could host two flagged visuals —
        // a flagged NPC body over a mount field — and double-stepping would square the decay).
        if seen.insert(node.unit) {
            // A unit riding a FLYING spline settles to level here: the flying attitude (pitch,
            // bank still open) is the spline-follow's, applied to the UNIT transform by
            // `sample_splines` — the client's `0x7c5490` mover-matrix law, model-flags-
            // independent (decision 0501 corrects 0496's conform-side placement, which never
            // rendered: the taxi gryphon authors `GlobalModelFlags = 0`, so it built no
            // ConformNode at all). The ground probe misses in the air → `tilt_target` yields
            // world-up, easing any flagged flier level.
            let target = {
                let origin = unit_tf.translation + Vec3::Y * TILT_PROBE_UP;
                let sampled = spatial
                    .cast_ray_predicate(
                        origin,
                        Dir3::NEG_Y,
                        TILT_PROBE_UP + TILT_PROBE_DOWN,
                        true,
                        &filter,
                        &|e| decals.receives(e),
                    )
                    .map(|hit| hit.normal);
                tilt_target(sampled)
            };
            let s = ups
                .entry(node.unit)
                .or_insert_with(|| target.unwrap_or(Vec3::Y));
            if let Some(t) = target {
                *s = t + (*s - t) * decay;
            }
        }
        let Some(&s) = ups.get(&node.unit) else {
            continue;
        };
        let n_local = unit_tf.rotation.inverse() * s;
        node_tf.rotation = conform_rotation(node.mode, n_local);
        count += 1;
        let pitch = n_local.z.atan2(n_local.y);
        if max_pitch.is_none_or(|(_, p)| pitch.abs() > p.abs()) {
            max_pitch = Some((node.unit, pitch));
        }
    }
    ups.retain(|e, _| seen.contains(e));
    let now = time.elapsed_secs();
    if now >= *census_at {
        *census_at = now + 1.0;
        if let Some((e, p)) = max_pitch {
            debug!(
                "terrain conform: {count} conforming units, max pitch {p:+.3} rad on {e} ({} tracked)",
                ups.len(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The producer + bridge gates (`0x637140` / `0x614cd0`): a miss and a near-vertical face
    /// target world-up, the ~69°..~89° band freezes, a walkable face tracks its normal.
    #[test]
    fn tilt_target_bands_match_the_byte_gates() {
        assert_eq!(tilt_target(None), Some(Vec3::Y));
        assert_eq!(tilt_target(Some(Vec3::new(1.0, 0.01, 0.0))), Some(Vec3::Y));
        assert_eq!(tilt_target(Some(Vec3::new(0.9, 0.2, 0.0))), None);
        let n = Vec3::new(0.0, 0.9, 0.3).normalize();
        assert_eq!(tilt_target(Some(n)), Some(n));
    }

    /// The `0x7106c0` flag dispatch: level is identity in every mode; flag 1 pitches nose-up on
    /// an uphill normal and discards roll; flag 3 carries the normal verbatim as the model's up
    /// (roll included) and reduces to flag 1 on a roll-free normal; flags 0/2 stay level.
    #[test]
    fn conform_rotation_matches_the_flag_dispatch() {
        for mode in [0u8, 1, 2, 3] {
            let q = conform_rotation(mode, Vec3::Y);
            assert!(
                q.angle_between(Quat::IDENTITY) < 1e-3,
                "level is identity (mode {mode})"
            );
        }
        let uphill = Vec3::new(0.0, 0.9, 0.3).normalize();
        let q1 = conform_rotation(1, uphill);
        let fwd = q1 * Vec3::NEG_Z;
        assert!(fwd.y > 0.05, "flag 1 uphill tips the nose up, got {fwd}");
        let side = Vec3::new(0.3, 0.9, 0.0).normalize();
        assert!(
            conform_rotation(1, side).angle_between(Quat::IDENTITY) < 1e-3,
            "flag 1 discards pure roll"
        );
        let q3 = conform_rotation(3, side);
        assert!(
            (q3 * Vec3::Y - side).length() < 1e-4,
            "flag 3 up = the normal verbatim"
        );
        assert!(
            conform_rotation(3, uphill).angle_between(q1) < 1e-3,
            "flag 3 reduces to flag 1 without roll"
        );
        assert!(conform_rotation(2, uphill).angle_between(Quat::IDENTITY) < 1e-3);
    }
}
