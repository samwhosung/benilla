//! **The spyglass zoom** — `SPELL_AURA_FAR_SIGHT` (aura 76), which is not far sight at all.
//!
//! The Ornate Spyglass (item 5507 → spell 12883 "Longsight") reads like a member of the far-sight
//! family and is filed with it in the bug ledger, but the two share only a name. Mind Vision and
//! Sentry Totem are `SPELL_AURA_BIND_SIGHT`, a *server* mechanism: the server moves its own
//! viewpoint and names the object in `PLAYER_FARSIGHT` ([`super::view_subject`]). Aura 76 is a
//! **client-local camera override** and never touches that field — which is why the server appears
//! to do nothing for the spyglass, and why chasing it on the wire finds no packet to handle.
//!
//! The reference's aura watcher `0x604d00` routes to `0x5ff350` (add) / `0x612320` (remove);
//! both, **for the local player only** (`0x5fa6d0`), walk the spell's three effects comparing
//! `EffectApplyAuraName[i]` (`SpellRec+0x16c`) against `0x4c` = 76 and call
//! `0x50d320(camera, EffectMiscValue[i])`. That function forces **first person**, sets a camera flag
//! (`|= 0x8`) that makes `SetCameraView` early-return — so the view is *locked* — and writes
//! `[camera+0x40] = n × π/180`. On removal it restores `π/2` and unlocks.
//!
//! So the spyglass is a locked first-person view at a **6× zoom**: `EffectMiscValue = 15` against a
//! stored default of 90.
//!
//! **We copy the ratio, not the number, and that is deliberate.** The reference's `[camera+0x40]`
//! and our [`CAM_FOVY`] are different quantities: theirs defaults to 90°, while ours is the
//! *effective vertical* field of view and is 45° (≈ the reference's measured 44.1°). Writing 15°
//! straight into our projection would be a third again too narrow. `15/90` is exact, dimensionless,
//! and survives whatever our FOV constant is — the same reasoning `camera.rs` uses for the
//! click-travel constants, where copying the reference's raw device units would have bound us to
//! its mouse scaling.

use bevy::camera::{PerspectiveProjection, Projection};
use bevy::prelude::*;

use crate::net::{ObjectStore, SelfPlayer};
use crate::ui_action::Spells;
use benilla_world::view::{WorldCamera, CAM_FOVY};

/// `SPELL_AURA_FAR_SIGHT` — `0x4c`, the value the reference's effect walk compares against.
const SPELL_AURA_FAR_SIGHT: u32 = 76;

/// The reference's unzoomed `[camera+0x40]`, in degrees (`π/2`, its ctor default at `0x50a706`).
/// Only ever a **denominator** here — see the module note on why we take the ratio.
const REFERENCE_DEFAULT_DEGREES: f32 = 90.0;

/// The scope currently held, as a fraction of the normal field of view (`1.0` = unzoomed).
///
/// `None` is the overwhelmingly common case and means "no override at all" — distinct from
/// `Some(1.0)`, which would still lock the camera into first person.
#[derive(Resource, Default)]
pub(crate) struct ScopedView {
    pub(crate) zoom: Option<f32>,
}

impl ScopedView {
    /// True while a scope is held — the camera is locked into first person for as long as it is.
    pub(crate) fn active(&self) -> bool {
        self.zoom.is_some()
    }
}

/// Read our own aura slots for aura 76 and drive the projection from it.
///
/// Local player only, exactly as the reference gates it: aura 76 on somebody *else* is none of our
/// camera's business, and the field is public, so we would otherwise zoom whenever a nearby player
/// raised a spyglass.
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
                    // The misc value is read from the SAME effect index that carries the aura
                    // name — the reference indexes both by `i`. A spell whose aura 76 sits in
                    // effect 2 would otherwise be read against effect 0's unrelated misc value.
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

    /// The ratio, and the two ways a naive port gets it wrong: writing the reference's 15 straight
    /// into a projection whose default is 45°, or restoring to the reference's 90° instead of ours.
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
