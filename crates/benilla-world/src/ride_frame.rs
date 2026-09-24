//! The ride frame, `[CM2Model+0x17c]`: the world matrix of the transport a model is riding.
//!
//! World-mode particle clouds (`0x10` clear) and every ribbon store committed content in absolute
//! coordinates, so on a transport the reference stores them in the transport's frame and
//! re-projects them live at draw. One matrix enters at two inverse sites:
//!
//! ```text
//! A = translate(transport.pos) · Rz(transport.facing)    // 0x630ac0, NULL when not riding
//! BIRTH  0x7b5160:  rt+0x1fc = srcMx · A⁻¹      (A ≠ 0 and 0x100 clear; else verbatim)
//! DRAW   0x7b3d20:  0xcf5b68 = A · T · S        (0x100 clear, [ebp+8] ≠ 0; else T·S)
//! EDGE   0x7187f0:  on A changing NULL↔non-NULL, re-express every live particle's
//!                   position and velocity (0x7b5e60) and every ribbon's (0x7b7bc0)
//! ```
//!
//! `A` lives on the model, not the bone or emitter, and the reference copies the parent's pointer
//! to every child model each frame (`0x7142c1`, in the animate kernel `0x714260`), so
//! [`RideFrames`] walks the [`crate::model_fade::ParentModel`] chain.

use bevy::ecs::system::SystemParam;
use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::model_fade::{ParentModel, MAX_MODEL_CHAIN};

/// This model instance rides a transport: `[CM2Model+0x17c]`, held as the transport's entity so
/// the pose is read live. Set on the rider's own model; its [`ParentModel`] chain inherits it.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RideFrame(pub Entity);

/// `A = translate(pos) · Rz(facing)` from a transport's live pose: position and Z-facing only, all
/// `0x630ac0` builds it from (vtable `+0x14`, `+0x18`). Yaw-only, it leaves gravity frame-free.
pub fn ride_matrix(gt: &GlobalTransform) -> Affine3A {
    let (_, rot, translation) = gt.to_scale_rotation_translation();
    let yaw = rot.to_euler(EulerRot::YXZ).0;
    Affine3A::from_rotation_translation(Quat::from_rotation_y(yaw), translation)
}

/// A model instance's ride frame, looked up along its [`ParentModel`] chain: the reference's
/// per-frame copy of `[model+0x17c]` down the child list (`0x7142c1`).
#[derive(SystemParam)]
pub struct RideFrames<'w, 's> {
    chain: Query<'w, 's, (Option<&'static RideFrame>, Option<&'static ParentModel>)>,
}

/// `WOW_NO_RIDE_FRAME=1` resolves no ride frame, an A/B lever: a rider's effects then stream off
/// the deck.
fn disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("WOW_NO_RIDE_FRAME").is_some())
}

impl RideFrames<'_, '_> {
    /// The transport `instance` rides, inherited from its model chain.
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

/// The frame an emitter's committed content is stored in, and the edge detector behind the
/// reference's `0x7187f0` re-expression. The entity says whether the frame changed; the last
/// matrix outlives a despawned transport, since leaving re-expresses through it.
#[derive(Clone, Copy, Default)]
pub struct StoredFrame(Option<(Entity, Affine3A)>);

impl StoredFrame {
    /// Move the store to `now`, returning the transform every stored point is re-expressed by
    /// (directions take its rotation), or `None` while the transport is unchanged. Boarding folds
    /// `A⁻¹`, leaving the old `A`, and a transport-to-transport step both: the reference wires only
    /// the NULL↔non-NULL edges, since `SetMoveBase` clears before it sets.
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

    /// The live `A` stored content is expressed in; `None` in world space.
    pub fn matrix(&self) -> Option<Affine3A> {
        self.0.map(|(_, a)| a)
    }

    /// `stored → world`: the draw fold (`0xcf5b68 = A · T · S`).
    pub fn to_world(&self, p: Vec3) -> Vec3 {
        self.0.map_or(p, |(_, a)| a.transform_point3(p))
    }

    /// `stored → world` for a direction (a velocity): rotation only.
    pub fn dir_to_world(&self, v: Vec3) -> Vec3 {
        self.0.map_or(v, |(_, a)| a.transform_vector3(v))
    }

    /// The transport whose frame this store is expressed in.
    pub fn source(&self) -> Option<Entity> {
        self.0.map(|(e, _)| e)
    }

    /// The store's rotation, `Rz(facing)`, for a stored orientation (a model particle's quat).
    pub fn rotation(&self) -> Quat {
        self.0
            .map_or(Quat::IDENTITY, |(_, a)| Quat::from_mat3a(&a.matrix3))
    }

    /// `world → stored`: the birth fold (`rt+0x1fc = srcMx · A⁻¹`), for a world read that must
    /// come back (a ground-snap hit).
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

    /// A per-frame fold while riding would move the cloud with the deck twice.
    #[test]
    fn only_the_boundary_re_expresses() {
        let mut f = StoredFrame::default();
        assert!(f.retarget(None).is_none(), "ground → ground");
        let a = deck(Vec3::new(10.0, 5.0, 0.0), 0.5);
        assert!(f.retarget(Some(a)).is_some(), "board");
        assert!(f.retarget(Some(a)).is_none(), "riding, deck unmoved");
        // The same transport at a new pose is the same frame: the draw's live `A` carries it.
        let moved = (a.0, Affine3A::from_translation(Vec3::new(10.0, 40.0, 0.0)));
        assert!(f.retarget(Some(moved)).is_none(), "the lift rose");
        assert!(f.retarget(None).is_some(), "leave");
    }

    /// The birth and draw folds are inverses: `A · (A⁻¹ · p) == p`.
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
