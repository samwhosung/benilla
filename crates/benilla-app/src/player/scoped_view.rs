//! The spyglass zoom, `SPELL_AURA_FAR_SIGHT` (aura 76): a client-local camera override, unlike the
//! server's `SPELL_AURA_BIND_SIGHT` far sight ([`super::view_subject`]), so no packet carries it.
//! For the local player only (`0x5fa6d0`), the aura watcher `0x604d00` routes to `0x5ff350` (add)
//! and `0x612320` (remove), which match each effect's `EffectApplyAuraName` (`SpellRec+0x16c`)
//! against 76 and call `0x50d320(camera, EffectMiscValue[i])`: first person, locked (camera flag
//! `0x8` makes `SetCameraView` return early), with `[camera+0x40] = n × π/180`; removal restores
//! `π/2` and unlocks. The Ornate Spyglass (item 5507, spell 12883) passes 15, a 6× zoom.
//!
//! The ratio is applied to `[camera+0x40]`, which defaults to 90°; the world projection then turns
//! it into the vertical field of view by the frame's aspect (`0x5c3cc0`, [`benilla_world::view`]).

use bevy::prelude::*;

use crate::net::{ObjectStore, SelfPlayer};
use crate::ui_action::Spells;
use benilla_world::view::FovZoom;

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
    mut fov_zoom: ResMut<FovZoom>,
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

    // `stamp_world_fov` turns it into the field of view at the frame's aspect.
    let want = scoped.zoom.unwrap_or(1.0);
    if fov_zoom.0 != want {
        fov_zoom.0 = want;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spyglass_narrows_our_own_field_of_view_six_fold() {
        use benilla_world::view::world_fovy;
        let scoped = ScopedView {
            zoom: Some(15.0 / REFERENCE_DEFAULT_DEGREES),
        };
        let normal = world_fovy(16.0 / 9.0);
        let fov = normal * scoped.zoom.unwrap();
        assert!(
            (fov - normal / 6.0).abs() < 1e-6,
            "the spyglass is a 6x zoom off whatever the normal FOV is, not a literal 15 degrees"
        );
        assert!(
            (fov.to_degrees() - 44.1 / 6.0).abs() < 0.02,
            "…which lands at a sixth of the 16:9 field of view, not 15"
        );
        assert!(scoped.active());

        let none = ScopedView { zoom: None };
        assert_eq!(
            none.zoom.unwrap_or(1.0),
            1.0,
            "unscoped restores the reference's 90, never a stored default"
        );
        assert!(!none.active());
    }
}
