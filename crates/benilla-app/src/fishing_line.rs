//! The engine-drawn fishing line, rod tip to bobber, while a unit channels Fishing.
//!
//! The reference keeps one line per unit (`[unit+0xb4c]`, ctor `0x61f490`) while its
//! `UNIT_FIELD_CHANNEL_OBJECT` is a live FISHINGNODE GameObject, `UNIT_CHANNEL_SPELL` is nonzero
//! and the mainhand is a loaded fishing pole, for every visible fisher, not only the player. Here
//! it is drawn each frame those conditions hold.
//!
//! The geometry (`0x61f780`): 65 vertices lerped near to far with a fixed sag of
//! `0.5 * sin(pi * t)` world units, not scaled by length. Near is the pole's `$CCH` marker (the
//! bobber's own `$CCH` is never read); far is the bobber's position plus half its scaled bbox
//! height. One flat color, the pole's light-collector ambient, opaque and unlit.
//!
//! Here the anchor rides the unposed prop frame, the color is the scene ambient rather than the
//! pole's light collector, and the line is unfogged. The sheathe trigger (`0x60d2f0`) and the
//! FishingCast to FishingLoop handoff (`0x5fc3f0` case 0x85) belong to the channel animations.

use bevy::prelude::*;

use benilla_protocol::EntityKind;

use crate::entities::OverheadFallback;
use crate::net::{GuidIndex, NetEntity, ObjectStore};

/// Marks a mainhand prop whose model authors `$CCH`, which only the fishing pole does, standing
/// in for the reference's `{class 2, subclass 20}` item check. Despawned with the prop.
#[derive(Component)]
pub(crate) struct FishingPoleTip {
    /// The unit holding the pole, whose channel fields decide whether a line draws.
    pub(crate) owner: Entity,
    /// `$CCH` in the prop's mesh frame (Bevy space).
    pub(crate) tip: Vec3,
}

/// 64 segments, 65 vertices: `t = i/64` (const `0x80a92c` = 0.015625).
const SEGMENTS: usize = 64;
/// The half-sine sag amplitude (const `0x80c9a8` = -0.5, applied to `sin(pi * t)`).
const SAG: f32 = 0.5;

/// Draws every visible fisher's line from this frame's prop frame.
fn draw_fishing_lines(
    poles: Query<(&FishingPoleTip, &GlobalTransform, &InheritedVisibility)>,
    owners: Query<&ObjectStore>,
    index: Res<GuidIndex>,
    bobbers: Query<(
        &ObjectStore,
        &NetEntity,
        &GlobalTransform,
        Option<&OverheadFallback>,
    )>,
    lighting: Option<Res<benilla_world::lighting::WowLighting>>,
    mut gizmos: Gizmos,
) {
    for (pole, prop, vis) in &poles {
        if !vis.get() {
            continue;
        }
        // The create conditions (`0x612650`): a channel spell aimed at a streamed FISHINGNODE.
        let Ok(store) = owners.get(pole.owner) else {
            continue;
        };
        if store.0.unit_channel_spell() == 0 {
            continue;
        }
        let Some(bobber) = store
            .0
            .unit_channel_object()
            .and_then(|g| index.0.get(&g).copied())
        else {
            continue;
        };
        let Ok((go_store, net, go_tf, height)) = bobbers.get(bobber) else {
            continue;
        };
        if net.kind != EntityKind::GameObject
            || go_store.0.gameobject_type_id() != crate::target::cursor_mode::GO_TYPE_FISHINGNODE
        {
            continue;
        }
        let near = prop.transform_point(pole.tip);
        // Bobber base plus half its scaled bbox height (`0x5f9f50`).
        let far = go_tf.translation() + Vec3::Y * (net.scale * height.map_or(0.0, |h| h.0) * 0.5);
        // The scene ambient clamped to [0, 1], opaque.
        let color = lighting.as_deref().map_or(Color::srgb(0.5, 0.5, 0.5), |l| {
            Color::srgb(
                l.ambient[0].clamp(0.0, 1.0),
                l.ambient[1].clamp(0.0, 1.0),
                l.ambient[2].clamp(0.0, 1.0),
            )
        });
        gizmos.linestrip(
            (0..=SEGMENTS).map(|i| {
                let t = i as f32 / SEGMENTS as f32;
                near.lerp(far, t) - Vec3::Y * (SAG * (std::f32::consts::PI * t).sin())
            }),
            color,
        );
    }
}

/// Registers the line drawer after transform propagation.
pub(crate) struct FishingLinePlugin;

impl Plugin for FishingLinePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            draw_fishing_lines.in_set(benilla_world::billboard::BillboardPlace),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference's polyline (`0x61f780`): zero sag at both ends, -0.5 in world Z (Bevy Y) at
    /// the midpoint.
    #[test]
    fn the_sag_is_a_fixed_half_sine() {
        let near = Vec3::new(0.0, 5.0, 0.0);
        let far = Vec3::new(20.0, 4.0, 0.0);
        let pts: Vec<Vec3> = (0..=SEGMENTS)
            .map(|i| {
                let t = i as f32 / SEGMENTS as f32;
                near.lerp(far, t) - Vec3::Y * (SAG * (std::f32::consts::PI * t).sin())
            })
            .collect();
        assert_eq!(pts.len(), 65);
        assert!((pts[0] - near).length() < 1e-6);
        assert!((pts[64] - far).length() < 1e-5);
        let mid = near.lerp(far, 0.5);
        assert!((pts[32].y - (mid.y - SAG)).abs() < 1e-4);
        // A 200 yd span dips the same 0.5.
        let long = near.lerp(Vec3::new(200.0, 5.0, 0.0), 0.5);
        let dip = SAG * (std::f32::consts::PI * 0.5).sin();
        assert!((dip - SAG).abs() < 1e-6);
        assert!(long.y - dip < long.y);
    }
}
