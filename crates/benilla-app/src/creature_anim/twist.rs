//! The **display-facing counter-twist** — the client's strafe/look body pose (the `0x607ed0` tail →
//! the `0x711f10` bone channels).
//!
//! A unit's rendered root yaw can sit *offset* from its aim — a strafe turns the root toward the
//! slide (±90° pure, ±45° diagonal) while the aim (camera, server orientation) holds. The client
//! then counter-rotates two key-bone subtrees back toward the aim, proportional to the remaining
//! gap: **SpineLow (KeyBoneID 4)** takes half the gap capped at 45°, **Head (KeyBoneID 6)** takes
//! the remainder capped at 45°. For a pure 90° strafe that composes to: hips/legs fully at the
//! strafe heading, shoulders counter-twisted back ~45°, and the head landing *exactly* on the aim
//! (45° + 45° = 90°) — nothing tuned, the arithmetic closes. The gap owner ([`crate::player`]'s
//! controller for our avatar, [`crate::net::motion`] for remote movers) writes [`BodyTwist::yaw_gap`];
//! the [`apply_body_twist`] system composes the twist onto the animated bone locals each frame,
//! after Bevy's animation evaluation and before transform propagation.

use bevy::prelude::*;

/// The body's counter-twist state, on a skinned unit's root entity. Inserted at visual attach when
/// the model carries either twist key-bone ([`benilla_assets::ModelSkeleton::spine_bone`]/
/// [`head_bone`](benilla_assets::ModelSkeleton::head_bone)); absent on beasts/props (the client's
/// capability gates `[+0xd58] & 0x80/0x100` — a model without the key-bone plays no channel).
#[derive(Component)]
pub(crate) struct BodyTwist {
    /// `wrap(aim − rendered root yaw)`, radians — how far the aim sits from the heading the model
    /// renders at. Zero whenever the body faces its aim (everything but a strafe, today).
    pub(crate) yaw_gap: f32,
    spine: Option<Channel>,
    head: Option<Channel>,
}

impl BodyTwist {
    pub(crate) fn new(spine: Option<u16>, head: Option<u16>) -> Self {
        Self {
            yaw_gap: 0.0,
            spine: spine.map(Channel::new),
            head: head.map(Channel::new),
        }
    }
}

/// One twist channel's bone + composition bookkeeping.
struct Channel {
    bone: u16,
    /// The animated local rotation the twist last composed on — the "base" under our twist.
    base: Quat,
    /// What we last wrote (`base * twist`). If the bone still holds exactly this next frame, the
    /// animation didn't retouch it this frame (a clip need not key every bone), so `base`
    /// stays authoritative — composing onto the bone's current value instead would accumulate the
    /// twist frame over frame and spin it.
    last_out: Quat,
}

impl Channel {
    fn new(bone: u16) -> Self {
        Self {
            bone,
            base: Quat::IDENTITY,
            last_out: Quat::IDENTITY,
        }
    }
}

/// Wrap an angle to `(−π, π]` — the shortest-arc form every yaw-gap computation here uses.
pub(crate) fn wrap_pi(angle: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    PI - (PI - angle).rem_euclid(TAU)
}

/// Split a yaw gap into the (spine, head) counter-twist angles — the client's share math
/// (`0x607ed0` tail, byte-VERIFIED): spine = half the gap capped at ±45°, head = the remainder
/// capped at ±45° — so a pure 90° strafe composes to 45°+45° and the head lands exactly on the aim.
///
/// The binary carries a full-share branch too (`0x6103a0`: local player AND a live click-to-move
/// action), but `[0xc4d888]` is the click-to-move action type and `0xc` = disabled is its normal
/// in-world value — so half is the effective share for everyone in
/// ordinary play, exactly as the director's reference eye called it when the full-share variant
/// was tried and rejected. Full share would fire only during click-to-move, which
/// benilla doesn't have.
fn twist_shares(gap: f32) -> (f32, f32) {
    use std::f32::consts::FRAC_PI_4;
    let spine = (gap * 0.5).clamp(-FRAC_PI_4, FRAC_PI_4);
    let head = (gap - spine).clamp(-FRAC_PI_4, FRAC_PI_4);
    (spine, head)
}

/// The shares actually ARMED on a unit — [`twist_shares`] with the **mounted spine gate** applied.
///
/// The SpineLow channel is armed only on an UNMOUNTED unit: `CGUnit+0xdc == 0` (in the `0x607ed0`
/// tail, alongside the per-model capability bit `+0xd58 & 0x80` we model as
/// "the skeleton authors the key bone"). `+0xdc` is the **mount model** pointer — the same field
/// `0x614cd0` passes to `0x7106c0` as "model `[+0xd8]` or mount `[+0xdc]`". The HEAD channel
/// (`+0xd58 & 0x100`) carries no mount gate. So a strafing rider counter-twists its head alone
/// while the shoulders ride the saddle rigidly; applying both wobbled a mounted strafe's head AND
/// shoulders far past the reference (the director's mounted A/B — the observation that found this).
///
/// The gate is on the channel ARM, not on the share math, so the head keeps its full `gap − spine`
/// share and — unlike the unmounted case — no longer lands exactly on the aim. And the spine's
/// share goes to **zero** rather than the channel being skipped: the zero path rewrites the bone
/// back to its animated base, which is the reference's disarm (`0x711f10(4, 0, 0x80)`); skipping
/// would freeze our last twist into any frame the clip does not re-key.
///
/// (`+0xdc`'s semantics are inferred. It is taken here because the mechanism
/// PREDICTS the reference: the director reported the mounted over-wobble before this gate was
/// found, and it is the gate that accounts for it.)
fn armed_shares(gap: f32, mounted: bool) -> (f32, f32) {
    let (spine, head) = twist_shares(gap);
    (if mounted { 0.0 } else { spine }, head)
}

/// Compose the counter-twist onto the animated bone locals — PostUpdate, in the pose post-pass
/// window ([`benilla_world::rig_anim::PosePost`]: after the evaluator wrote this frame's pose, before the model
/// compose folds it).
///
/// Each channel yaws its subtree about **world up through the bone's own pivot**: with `g` the
/// bone's model-space rotation (ancestors × its animated local), `local' = local · Quat(g⁻¹·Y, θ)`
/// conjugates to a pure up-axis yaw of the subtree (units stand upright and their root rotation is
/// a Y-yaw, so model up and world up coincide). The head channel runs after the spine write, so its
/// ancestor chain already carries the spine's twist — the head counter-rotates relative to the
/// twisted spine, exactly the client's residual-gap composition. The ancestor walk runs up the
/// rig's own parent table, then the entity frames between its `joints_root` and the unit (a
/// conform node's tilt; a mounted rider's seat — splicing through the mount's bone chain via its
/// [`benilla_world::rig_anim::RigAnchor`]), exactly the frames the joint-entity walk used to compose.
pub(super) fn apply_body_twist(
    // A parked rig's bones are frozen — composing the twist onto them would
    // recompute the palette every frame for a unit no one sees; the wake re-seats `base` from the
    // fresh sample on its own (`cur != last_out`).
    mut units: Query<(Entity, &mut BodyTwist), Without<benilla_world::rig_anim::AnimParked>>,
    mut rigs: Query<&mut benilla_world::rig_anim::RigPose>,
    anchors: Query<&benilla_world::rig_anim::RigAnchor>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
    stores: Query<&crate::net::ObjectStore>,
) {
    // `WOW_NO_TWIST=1` — stand the whole counter-twist down, and say what the gap WAS. The pass
    // is inert at `yaw_gap == 0` (both shares are zero and every write is a no-op), so a bisect
    // that only removes it cannot tell "the twist was innocent" from "the twist never ran"; the
    // logged gap separates them.
    if twist_off() {
        for (_, t) in &units {
            if t.yaw_gap != 0.0 {
                info!(
                    "twist: STOOD DOWN (WOW_NO_TWIST) — yaw_gap was {:.5} rad",
                    t.yaw_gap
                );
            }
        }
        return;
    }
    for (unit, mut twist) in &mut units {
        let mounted = stores
            .get(unit)
            .is_ok_and(|s| s.0.unit_mount_display_id() != 0);
        let (spine, head) = armed_shares(twist.yaw_gap, mounted);
        let twist = &mut *twist;
        for (channel, angle) in [(&mut twist.spine, spine), (&mut twist.head, head)] {
            let Some(ch) = channel else { continue };
            let bone = ch.bone as usize;
            let Some((cur, base, out)) = ({
                let rig = rigs.get(unit).ok();
                rig.and_then(|rig| {
                    let cur = rig.locals.get(bone)?.rotation;
                    let base = if cur == ch.last_out { ch.base } else { cur };
                    let out = if angle == 0.0 {
                        base
                    } else {
                        // The bone's model-space rotation: its own ancestor chain…
                        let mut g = base;
                        let mut b = rig.parents.get(bone).copied().unwrap_or(-1);
                        while let Ok(p) = usize::try_from(b) {
                            g = rig.locals.get(p)?.rotation * g;
                            b = rig.parents.get(p).copied().unwrap_or(-1);
                        }
                        // …then the frames between joints_root and (excluding) the unit, splicing
                        // a rig anchor's bone chain (the mount seat). Depth-capped against a
                        // malformed hierarchy; the joint walk bottomed out at the world root.
                        let mut e = rig.joints_root;
                        for _ in 0..32 {
                            if e == unit {
                                break;
                            }
                            if let Some(host) = anchors
                                .get(e)
                                .ok()
                                .and_then(|a| rigs.get(a.rig).ok().map(|r| (r, a.bone)))
                            {
                                let (host_rig, hb) = host;
                                let mut b = Ok(hb as usize);
                                while let Ok(p) = b {
                                    let Some(t) = host_rig.locals.get(p) else {
                                        break;
                                    };
                                    g = t.rotation * g;
                                    b = usize::try_from(
                                        host_rig.parents.get(p).copied().unwrap_or(-1),
                                    );
                                }
                                e = host_rig.joints_root;
                                continue;
                            }
                            if let Ok(t) = locals.get(e) {
                                g = t.rotation * g;
                            }
                            let Ok(p) = parents.get(e).map(|c| c.parent()) else {
                                break;
                            };
                            e = p;
                        }
                        (base * Quat::from_axis_angle(g.inverse() * Vec3::Y, angle)).normalize()
                    };
                    Some((cur, base, out))
                })
            }) else {
                continue;
            };
            ch.base = base;
            ch.last_out = out;
            if out != cur {
                if let Ok(mut rig) = rigs.get_mut(unit) {
                    if let Some(t) = rig.locals.get_mut(bone) {
                        t.rotation = out;
                        rig.pose_dirty = true;
                    }
                }
            }
        }
    }
}

/// Register [`apply_body_twist`] in the pose post-pass window.
pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        PostUpdate,
        apply_body_twist.in_set(benilla_world::rig_anim::PosePost),
    );
}

/// `WOW_NO_TWIST`'s switch — read once. A bisect lever, not a shipping mode.
fn twist_off() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("WOW_NO_TWIST").is_some())
}

#[cfg(test)]
mod tests {
    use super::{armed_shares, twist_shares};
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    /// **A mounted rider counter-twists its HEAD only** — the `CGUnit+0xdc == 0` gate on the
    /// SpineLow channel. The head's share is untouched by the gate (it is on the channel arm, not
    /// the share math), so the two no longer sum to the gap and the head sits off the aim.
    #[test]
    fn mounted_disarms_the_spine_and_leaves_the_head_share_alone() {
        let (spine, head) = armed_shares(-FRAC_PI_2, true);
        assert_eq!(spine, 0.0, "the saddle holds the shoulders rigid");
        assert_eq!(
            head, -FRAC_PI_4,
            "the head keeps its full gap − spine share"
        );
        // Unmounted, the same gap arms both — the 45°+45° close.
        assert_eq!(armed_shares(-FRAC_PI_2, false), twist_shares(-FRAC_PI_2));
    }

    #[test]
    fn pure_strafe_gap_closes_exactly_at_the_head() {
        // 90° gap: spine 45°, head 45° — the head lands back on the aim.
        let (spine, head) = twist_shares(-FRAC_PI_2);
        assert_eq!(spine, -FRAC_PI_4);
        assert_eq!(head, -FRAC_PI_4);
        assert_eq!(spine + head, -FRAC_PI_2);
    }

    #[test]
    fn diagonal_strafe_splits_evenly() {
        // 45° gap: spine 22.5°, head 22.5° — head on the aim again (the half share, everywhere:
        // the director's reference eye rejected the binary's local-player full-share branch).
        let (spine, head) = twist_shares(FRAC_PI_4);
        assert_eq!(spine, FRAC_PI_4 / 2.0);
        assert_eq!(head, FRAC_PI_4 / 2.0);
    }

    #[test]
    fn shares_cap_at_45_degrees_each() {
        // An extreme gap (π) can't be fully absorbed: both channels cap at 45°.
        let (spine, head) = twist_shares(PI);
        assert_eq!(spine, FRAC_PI_4);
        assert_eq!(head, FRAC_PI_4);
    }

    #[test]
    fn zero_gap_is_zero_twist() {
        assert_eq!(twist_shares(0.0), (0.0, 0.0));
    }

    #[test]
    fn wrap_pi_takes_the_shortest_arc() {
        use super::wrap_pi;
        assert_eq!(wrap_pi(0.0), 0.0);
        assert!((wrap_pi(3.0 * FRAC_PI_2) + FRAC_PI_2).abs() < 1e-6);
        assert!((wrap_pi(-3.0 * FRAC_PI_2) - FRAC_PI_2).abs() < 1e-6);
        assert_eq!(wrap_pi(PI), PI);
    }
}
