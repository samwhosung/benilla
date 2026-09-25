//! The spyglass zoom, `SPELL_AURA_FAR_SIGHT` (aura 76): a client-local camera override, unlike the
//! server's `SPELL_AURA_BIND_SIGHT` far sight ([`super::view_subject`]), so no packet carries it.
//! For the local player only (`0x5fa6d0`), the aura watcher `0x604d00` routes to `0x5ff350` (add)
//! and `0x612320` (remove), which match each effect's `EffectApplyAuraName` (`SpellRec+0x16c`)
//! against 76 and call `0x50d320(camera, EffectMiscValue[i])`: first person, locked (camera flag
//! `0x8` makes `SetCameraView` return early), with `[camera+0x40] = n × π/180`; removal restores
//! `π/2` and unlocks. The Ornate Spyglass (item 5507, spell 12883) passes 15, a 6× zoom.
//!
//! The ratio is applied, not the degrees: `[camera+0x40]` defaults to 90°, while our [`CAM_FOVY`]
//! is the effective vertical field of view, 45° against the reference's measured 44.1°.

use bevy::camera::{PerspectiveProjection, Projection};
use bevy::prelude::*;

use crate::net::{ObjectStore, SelfPlayer};
use crate::ui_action::Spells;
use benilla_world::view::{WorldCamera, CAM_FOVY};

/// `0x4c`, the aura name the reference's effect walk matches.
const SPELL_AURA_FAR_SIGHT: u32 = 76;

/// The reference's unzoomed `[camera+0x40]` in degrees (`π/2`, its constructor's value at
/// `0x50a706`), used only as a denominator.
const REFERENCE_DEFAULT_DEGREES: f32 = 90.0;

/// The held scope as a fraction of the normal field of view. `None` is no override;
/// `Some(1.0)` would still lock first person.
#[derive(Resource, Default)]
pub(crate) struct ScopedView {
    pub(crate) zoom: Option<f32>,
}

impl ScopedView {
    /// While a scope is held the camera is locked in first person.
    pub(crate) fn active(&self) -> bool {
        self.zoom.is_some()
    }
}

/// Drives the projection from aura 76 in our own aura slots only, as the reference gates it: the
/// aura field is public, so another player's spyglass must not zoom ours.
pub(super) fn apply_scoped_view(
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    spells: Option<Res<Spells>>,
    mut scoped: ResMut<ScopedView>,
    mut projections: Query<&mut Projection, With<WorldCamera>>,
) {
    let catalog = spells.as_ref().map(|s| &s.catalog);
    scoped.zoom = self_q
        .single()
        .ok()
        .zip(catalog)
        .and_then(|(store, catalog)| {
            store.0.unit_auras().find_map(|slot| {
                let rec = catalog.get(slot.spell_id)?;
                (0..3).find_map(|i| {
                    // The reference reads the misc value from the aura name's own effect index.
                    (rec.effect_apply_aura[i] == SPELL_AURA_FAR_SIGHT)
                        .then(|| rec.effect_misc_value[i] as f32)
                })
            })
        })
        // A zero misc value is the reference's own "restore" argument, not a zero-width view.
        .filter(|&deg| deg > 0.0)
        .map(|deg| deg / REFERENCE_DEFAULT_DEGREES);

    let Ok(mut projection) = projections.single_mut() else {
        return;
    };
    let want = CAM_FOVY * scoped.zoom.unwrap_or(1.0);
    if let Projection::Perspective(PerspectiveProjection { fov, .. }) = &mut *projection {
        if (*fov - want).abs() > f32::EPSILON {
            *fov = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spyglass_narrows_our_own_field_of_view_six_fold() {
        let scoped = ScopedView {
            zoom: Some(15.0 / REFERENCE_DEFAULT_DEGREES),
        };
        let fov = CAM_FOVY * scoped.zoom.unwrap();
        assert!(
            (fov - CAM_FOVY / 6.0).abs() < 1e-6,
            "the spyglass is a 6x zoom off whatever our normal FOV is, not a literal 15 degrees"
        );
        assert!(
            (fov.to_degrees() - 7.5).abs() < 1e-4,
            "…which lands at 7.5 degrees vertical for our 45 degree default, not 15"
        );
        assert!(scoped.active());

        let none = ScopedView { zoom: None };
        assert_eq!(
            CAM_FOVY * none.zoom.unwrap_or(1.0),
            CAM_FOVY,
            "unscoped restores OUR default, never the reference's stored 90"
        );
        assert!(!none.active());
    }
}
