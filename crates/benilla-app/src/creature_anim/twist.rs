//! The strafe counter-twist (the `0x607ed0` tail → the `0x711f10` bone channels): a strafe turns
//! the root toward the slide while the aim holds, and SpineLow (KeyBoneID 4) turns back half the
//! gap and Head (KeyBoneID 6) the rest, each capped at 45°, so a pure strafe's head lands on the
//! aim. [`crate::player`] and [`crate::net::motion`] write [`BodyTwist::yaw_gap`].

use bevy::prelude::*;

/// The twist state, on a unit's root when its model has either key bone (the client's
/// `[+0xd58] & 0x80`/`0x100`).
#[derive(Component)]
pub(crate) struct BodyTwist {
    /// `wrap(aim − rendered root yaw)` in radians; zero while the body faces its aim.
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

/// One twist channel's bone and bookkeeping.
struct Channel {
    bone: u16,
    /// The animated local rotation the twist last composed on.
    base: Quat,
    /// What we last wrote (`base * twist`). A bone still holding it was not re-keyed this frame, so
    /// `base` stays; composing onto it would accumulate the twist and spin the bone.
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

/// Wrap an angle to `(−π, π]`, the shortest arc.
pub(crate) fn wrap_pi(angle: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    PI - (PI - angle).rem_euclid(TAU)
}

/// The `(spine, head)` twist angles. The reference's full-share branch (`0x6103a0`) fires only for
/// the local player during a click-to-move action (`[0xc4d888]` holds `0xc`, disabled, in
/// ordinary play), which benilla does not have.
fn twist_shares(gap: f32) -> (f32, f32) {
    use std::f32::consts::FRAC_PI_4;
    let spine = (gap * 0.5).clamp(-FRAC_PI_4, FRAC_PI_4);
    let head = (gap - spine).clamp(-FRAC_PI_4, FRAC_PI_4);
    (spine, head)
}

/// [`twist_shares`] with the mount gate: the `0x607ed0` tail arms SpineLow only while
/// `CGUnit+0xdc`, the mount model (set by `0x613d80`, read as the mount by `0x614cd0`), is null,
/// and Head always, so a rider's head keeps its share and stops short of the aim. A mounted spine
/// share is zero, not skipped: the zero write restores the animated base, the reference's disarm
/// (`0x711f10(4, 0, 0x80)`), where skipping would freeze the last twist.
fn armed_shares(gap: f32, mounted: bool) -> (f32, f32) {
    let (spine, head) = twist_shares(gap);
    (if mounted { 0.0 } else { spine }, head)
}

/// Compose the twist onto the bone locals in [`benilla_world::rig_anim::PosePost`]: each channel
/// yaws its subtree about world up through the bone's pivot, `local' = local · Quat(g⁻¹·Y, θ)`
/// with `g` the bone's rotation up to the unit. The head runs after the spine, so it turns relative
/// to the twisted spine, as the client composes the residual gap.
pub(super) fn apply_body_twist(
    // A parked rig is skipped; on wake, `cur != last_out` re-seats `base`.
    mut units: Query<(Entity, &mut BodyTwist), Without<benilla_world::rig_anim::AnimParked>>,
    mut rigs: Query<&mut benilla_world::rig_anim::RigPose>,
    anchors: Query<&benilla_world::rig_anim::RigAnchor>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
    stores: Query<&crate::net::ObjectStore>,
) {
    // `WOW_NO_TWIST=1` stands the pass down and logs the gap it would have closed.
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
                        // The bone's rotation: its own ancestor chain…
                        let mut g = base;
                        let mut b = rig.parents.get(bone).copied().unwrap_or(-1);
                        while let Ok(p) = usize::try_from(b) {
                            g = rig.locals.get(p)?.rotation * g;
                            b = rig.parents.get(p).copied().unwrap_or(-1);
                        }
                        // …then the frames from joints_root up to the unit, splicing a rig
                        // anchor's bone chain (the mount seat); capped against a bad hierarchy.
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

/// `WOW_NO_TWIST`, read once: a bisect lever.
fn twist_off() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("WOW_NO_TWIST").is_some())
}

#[cfg(test)]
mod tests {
    use super::{armed_shares, twist_shares};
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    /// The mount gate disarms SpineLow alone; the head keeps its share, off the aim.
    #[test]
    fn mounted_disarms_the_spine_and_leaves_the_head_share_alone() {
        let (spine, head) = armed_shares(-FRAC_PI_2, true);
        assert_eq!(spine, 0.0, "the saddle holds the shoulders rigid");
        assert_eq!(
            head, -FRAC_PI_4,
            "the head keeps its full gap − spine share"
        );
        // Unmounted, the same gap arms both.
        assert_eq!(armed_shares(-FRAC_PI_2, false), twist_shares(-FRAC_PI_2));
    }

    #[test]
    fn pure_strafe_gap_closes_exactly_at_the_head() {
        // 90°: spine 45°, head 45°, the head back on the aim.
        let (spine, head) = twist_shares(-FRAC_PI_2);
        assert_eq!(spine, -FRAC_PI_4);
        assert_eq!(head, -FRAC_PI_4);
        assert_eq!(spine + head, -FRAC_PI_2);
    }

    #[test]
    fn diagonal_strafe_splits_evenly() {
        // 45°: spine 22.5°, head 22.5°, the head on the aim again.
        let (spine, head) = twist_shares(FRAC_PI_4);
        assert_eq!(spine, FRAC_PI_4 / 2.0);
        assert_eq!(head, FRAC_PI_4 / 2.0);
    }

    #[test]
    fn shares_cap_at_45_degrees_each() {
        // A gap of π cannot be absorbed: both channels cap at 45°.
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
