//! The **ride frame** — `[CM2Model+0x17c]`, the world matrix of the transport a model is riding.
//!
//! A world-mode particle cloud (`0x10` CLEAR) and *every* ribbon trail store their committed
//! content in absolute coordinates and never re-apply a live matrix to it. That is what makes a
//! trail: run past a torch and the smoke hangs where it was born. On a **transport** the same
//! rule would stream a rider's effects off the stern, so the reference stores them in the
//! transport's frame instead and re-projects them live at draw. The mechanism is one matrix
//! entering at exactly two sites, and they are inverses (decision 1591, which named this gap and
//! left it unbuilt):
//!
//! ```text
//! A = translate(transport.pos) · Rz(transport.facing)    // 0x630ac0 — position and Z-facing only
//!   = NULL for a model not riding anything
//!
//! BIRTH  0x7b5160:  rt+0x1fc = srcMx · A⁻¹      (A ≠ 0 and 0x100 CLEAR; else verbatim)
//! DRAW   0x7b3d20:  0xcf5b68 = A · T · S        (0x100 CLEAR, [ebp+8] ≠ 0; else T·S)
//! EDGE   0x7187f0:  on A changing NULL↔non-NULL, re-express every ALREADY-LIVE particle's
//!                   position and velocity (0x7b5e60) and every ribbon's (0x7b7bc0)
//! ```
//!
//! So a rider's effects are *stored* on the deck and *drawn* through the deck's live pose — they
//! hang still relative to the deck, and boarding or leaving re-expresses the cloud instead of
//! snapping it. The residual over a particle's life is exactly `A(t₀)⁻¹·A(t₁)`: the **transport's**
//! motion since birth, never the host unit's own.
//!
//! **Where `A` lives.** On the MODEL, not the bone and not the emitter — and the reference copies
//! the parent's pointer down to every child model each frame (`0x7142c1`, inside the animate
//! kernel `0x714260`), so
//! a held weapon's enchant streamer inherits its wearer's transport without knowing anything about
//! it. Our model child list is [`crate::model_fade::ParentModel`], so [`RideFrames`] walks that
//! same chain — one component on the rider, inherited by everything hung off it.

use bevy::ecs::system::SystemParam;
use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::model_fade::{ParentModel, MAX_MODEL_CHAIN};

/// **This model instance is riding a transport** — the reference's `[CM2Model+0x17c]`, as the
/// transport's entity rather than a copied matrix (the pose is read live, so a stored copy could
/// only go stale).
///
/// Written by the layer that knows who is standing on what (`benilla-app`'s transport module) on
/// the **rider's own model entity**; everything chained to it through [`ParentModel`] — held
/// items, their enchant glows, spell kits — inherits it through [`RideFrames`], exactly as the
/// reference's per-frame child copy does.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RideFrame(pub Entity);

/// `A = translate(pos) · Rz(facing)` from a transport's live pose — **position and Z-facing only**,
/// which is all `0x630ac0` builds it from (vtable `+0x14` position, `+0x18` Z-facing). Scale and
/// any pitch/roll are deliberately dropped: `A` is a rigid ride-frame, and its inverse is applied
/// to already-committed content, so anything non-rigid here would shear a live cloud.
///
/// Dropping pitch/roll is also what keeps gravity frame-free: world down is invariant under a yaw,
/// so a stored cloud integrates with the same `-Y` on a deck as on the ground.
pub fn ride_matrix(gt: &GlobalTransform) -> Affine3A {
    let (_, rot, translation) = gt.to_scale_rotation_translation();
    let yaw = rot.to_euler(EulerRot::YXZ).0;
    Affine3A::from_rotation_translation(Quat::from_rotation_y(yaw), translation)
}

/// Resolve a model instance's ride frame by walking the [`ParentModel`] chain — the reference's
/// per-frame propagation of `[model+0x17c]` down the child list (`0x7142c1`), as a lookup.
///
/// Read-only and cheap: the chains are 1–3 links (unit → item → glow) and the walk stops at the
/// first [`RideFrame`], so a world that is riding nothing pays one failed component fetch per
/// emitter.
#[derive(SystemParam)]
pub struct RideFrames<'w, 's> {
    chain: Query<'w, 's, (Option<&'static RideFrame>, Option<&'static ParentModel>)>,
}

/// `WOW_NO_RIDE_FRAME=1` — the **A/B lever** (the shape `WOW_NO_FRAME_PACE` and
/// `WOW_NO_RIG_REBASE` set): resolve no ride frame at all, which is the pre-1591 behaviour where a
/// rider's world-mode effects are stored against the world and stream off the back of the deck.
///
/// It earns its branch because this law is *only* observable on a moving transport: an A/B needs a
/// lift, a live login and a census on both sides, and without the lever the "before" half is a
/// second checkout and a cold build every time anyone asks. Read once — an emitter asks this
/// question every frame.
fn disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("WOW_NO_RIDE_FRAME").is_some())
}

impl RideFrames<'_, '_> {
    /// The transport `instance` rides, inherited from its model chain. `None` on the ground —
    /// which is every model in the world that is not standing on a boat, a zeppelin or a lift.
    pub fn source(&self, instance: Entity) -> Option<Entity> {
        if disabled() {
            return None;
        }
        let mut at = instance;
        for _ in 0..MAX_MODEL_CHAIN {
            let Ok((own, parent)) = self.chain.get(at) else {
                break;
            };
            if let Some(RideFrame(transport)) = own {
                return Some(*transport);
            }
            at = parent?.0;
        }
        None
    }
}

/// The frame an emitter's already-committed content is stored in, and the edge detector that keeps
/// it honest — the state behind the reference's `0x7187f0` re-expression.
///
/// Carries the transport's **entity** (the identity: has the frame changed?) *and* its last-known
/// **matrix** (the value: what a leave must re-express through, which must survive the transport
/// itself despawning).
#[derive(Clone, Copy, Default)]
pub struct StoredFrame(Option<(Entity, Affine3A)>);

impl StoredFrame {
    /// Move the store to `now`, returning the rigid transform every already-stored **point** must
    /// be re-expressed by (directions take its rotation) — `None` when the frame kept its identity,
    /// which is the steady state both on the ground and while riding.
    ///
    /// Boarding folds `A⁻¹` (world → deck), leaving folds the old `A` (deck → world), and stepping
    /// from one transport straight to another does both. The reference only wires the NULL↔non-NULL
    /// edges, because `SetMoveBase` clears before it sets; composing the two is the same answer for
    /// the case it never sees.
    pub fn retarget(&mut self, now: Option<(Entity, Affine3A)>) -> Option<Affine3A> {
        let fold = match (self.0, now) {
            (None, Some((_, a))) => Some(a.inverse()),
            (Some((_, old)), None) => Some(old),
            (Some((e0, old)), Some((e1, a))) if e0 != e1 => Some(a.inverse() * old),
            _ => None,
        };
        self.0 = now;
        fold
    }

    /// The live `A` stored content is expressed in — `None` in world space, where every fold in
    /// this module is the identity and costs nothing.
    pub fn matrix(&self) -> Option<Affine3A> {
        self.0.map(|(_, a)| a)
    }

    /// `stored → world`: the draw fold (`0xcf5b68 = A · T · S`).
    pub fn to_world(&self, p: Vec3) -> Vec3 {
        self.0.map_or(p, |(_, a)| a.transform_point3(p))
    }

    /// `stored → world` for a **direction** (a velocity): rotation only.
    pub fn dir_to_world(&self, v: Vec3) -> Vec3 {
        self.0.map_or(v, |(_, a)| a.transform_vector3(v))
    }

    /// The transport whose frame this store is expressed in — for instruments, which need to name
    /// the deck a trail is measured against.
    pub fn source(&self) -> Option<Entity> {
        self.0.map(|(e, _)| e)
    }

    /// The store's own rotation — `Rz(facing)` — for a stored ORIENTATION rather than a point
    /// (a 3-D model particle's instance quat).
    pub fn rotation(&self) -> Quat {
        self.0
            .map_or(Quat::IDENTITY, |(_, a)| Quat::from_mat3a(&a.matrix3))
    }

    /// `world → stored`: the birth fold (`rt+0x1fc = srcMx · A⁻¹`), for the one-off world reads
    /// that have to come back (a ground-snap probe hit).
    pub fn to_stored(&self, p: Vec3) -> Vec3 {
        self.0.map_or(p, |(_, a)| a.inverse().transform_point3(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deck(pos: Vec3, yaw: f32) -> (Entity, Affine3A) {
        (
            Entity::from_raw_u32(1).expect("a valid test entity id"),
            Affine3A::from_rotation_translation(Quat::from_rotation_y(yaw), pos),
        )
    }

    /// The steady states — on the ground and riding — must fold NOTHING. A re-expression that
    /// fired every frame while riding would drag the cloud along with the deck twice over, which is
    /// the rigid-ride bug the trail law exists to avoid.
    #[test]
    fn only_the_boundary_re_expresses() {
        let mut f = StoredFrame::default();
        assert!(f.retarget(None).is_none(), "ground → ground");
        let a = deck(Vec3::new(10.0, 5.0, 0.0), 0.5);
        assert!(f.retarget(Some(a)).is_some(), "board");
        assert!(f.retarget(Some(a)).is_none(), "riding, deck unmoved");
        // The SAME transport at a NEW pose is still the same frame: the draw's live `A` carries
        // the cloud, so nothing stored may be touched. This is the whole mechanism.
        let moved = (a.0, Affine3A::from_translation(Vec3::new(10.0, 40.0, 0.0)));
        assert!(f.retarget(Some(moved)).is_none(), "the lift rose");
        assert!(f.retarget(None).is_some(), "leave");
    }

    /// Boarding must leave a live particle exactly where it was in the world: the birth fold and
    /// the draw fold are inverses, so `A · (A⁻¹ · p) == p`.
    #[test]
    fn boarding_moves_nothing_on_screen() {
        let mut f = StoredFrame::default();
        let world = Vec3::new(-1286.0, 76.0, 189.0);
        let a = deck(Vec3::new(-1280.0, 60.0, 185.0), 1.1);
        let fold = f.retarget(Some(a)).expect("boarding re-expresses");
        let stored = fold.transform_point3(world);
        assert!(
            (f.to_world(stored) - world).length() < 1e-3,
            "stored {stored:?} draws back to {:?}, not {world:?}",
            f.to_world(stored)
        );
    }

    /// …and leaving is the same identity in the other direction, so stepping off a moving lift
    /// hands the cloud back to the world where it stood rather than snapping it to the origin.
    #[test]
    fn leaving_hands_the_cloud_back_in_place() {
        let mut f = StoredFrame::default();
        let a = deck(Vec3::new(-1280.0, 60.0, 185.0), 1.1);
        f.retarget(Some(a));
        let stored = Vec3::new(2.0, 1.5, -0.5);
        let world = f.to_world(stored);
        let fold = f.retarget(None).expect("leaving re-expresses");
        assert!((fold.transform_point3(stored) - world).length() < 1e-3);
    }
}
