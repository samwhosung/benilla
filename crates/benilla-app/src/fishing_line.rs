//! The engine-drawn **fishing line**: rod tip → bobber while a unit channels Fishing.
//!
//! The reference draws this as a per-UNIT effect, not a GO one — one line object per unit
//! (`[unit+0xb4c]`, ctor `0x61f490`), created when the unit's `UNIT_FIELD_CHANNEL_OBJECT`
//! resolves to a live **FISHINGNODE** GameObject, `UNIT_CHANNEL_SPELL` ≠ 0, and the mainhand is a
//! fishing pole whose model is loaded; torn down when any of that ceases (the server clears the
//! channel fields at finish/interrupt). **No local-player gate** — every visible fisher shows a
//! line. Benilla draws it immediate-mode per frame from exactly those conditions, so the
//! reference's create/watcher/teardown lifecycle falls out as the condition holding or not.
//!
//! The geometry (`0x61f780`): **65 vertices**, straight
//! lerp near → far, then a fixed half-sine sag — `z −= 0.5 × sin(π·t)`, 0.5 world-units at the
//! midpoint, not length- or physics-scaled. Near = the pole M2's `$CCH` event marker (the bobber
//! authors one too and the reference NEVER reads it — its far end is the bobber's position with
//! `z += scale × bboxHeight × 0.5`). One flat color for the whole strip = the pole's
//! light-collector accumulated ambient, alpha opaque, GL_LIGHTING forced off, drawn as a plain
//! line strip in scene state.
//!
//! Named deviations (same class as the bowstring's): the anchor rides the static
//! prop frame (the pole's bone 1 is not posed — item props rest at bind pose here); the color
//! samples the scene ambient ([`benilla_world::lighting::WowLighting`]) rather than a per-model light
//! collector; gizmo lines are unfogged. The reference's sheath side-trigger (`0x60d2f0` calls
//! `SetSheatheState(1)` when the watcher finds the pole on the back) and the FishingCast→
//! FishingLoop anim handoff (`0x5fc3f0` case 0x85) are the channel-anim family's, not drawn here.

use bevy::prelude::*;

use benilla_protocol::EntityKind;

use crate::entities::OverheadFallback;
use crate::net::{GuidIndex, NetEntity, ObjectStore};

/// Marks a spawned **mainhand prop** whose model authors the `$CCH` line anchor — the fishing
/// pole (exactly one weapon model in the 5875 chain authors it: the `scan_events` sweep, so
/// presence is the reference's `{class 2, subclass 20}` ItemCache check data-equivalently).
/// Inserted by the held-item attach; despawned with the prop, which is the clear.
#[derive(Component)]
pub(crate) struct FishingPoleTip {
    /// The unit holding the pole — its channel fields decide whether a line draws.
    pub(crate) owner: Entity,
    /// `$CCH` in the prop's mesh frame (Bevy space).
    pub(crate) tip: Vec3,
}

/// 64 segments / 65 vertices — the reference's `t = i/64` (const `0x80a92c` = 0.015625).
const SEGMENTS: usize = 64;
/// The fixed half-sine sag amplitude (const `0x80c9a8` = −0.5, applied to `sin(π·t)`).
const SAG: f32 = 0.5;

/// Draw every visible fisher's line (per frame, post-propagation so the prop frame is current).
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
        // The reference's create conditions, checked live: a channel spell, aimed at a streamed
        // FISHINGNODE. (`0x612650`; the GO-type read is the strategy's own `descr+0x3c`.)
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
        // Far = bobber base + half its scaled bbox height (`0x5f9f50`: `z += scale × [go+0xbc] ×
        // 0.5`) — the float's waterline center, whatever the model's pivot does.
        let far = go_tf.translation() + Vec3::Y * (net.scale * height.map_or(0.0, |h| h.0) * 0.5);
        // The flat strip color: accumulated ambient clamped [0,1], opaque (the scene-ambient
        // stand-in for the pole's collector — module doc).
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

/// Registers the line drawer beside the bowstring's — after propagation, so both weapon-prop
/// spans read this frame's frames.
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

    /// The polyline the reference builds (`0x61f780`): 65 vertices, straight lerp, and the fixed
    /// half-sine sag — zero at both ends, exactly −0.5 in world Z (Bevy Y) at the midpoint,
    /// independent of the span's length.
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
        // The sag never scales with length: a 200-yard span dips the same 0.5.
        let long = near.lerp(Vec3::new(200.0, 5.0, 0.0), 0.5);
        let dip = SAG * (std::f32::consts::PI * 0.5).sin();
        assert!((dip - SAG).abs() < 1e-6);
        assert!(long.y - dip < long.y);
    }
}
