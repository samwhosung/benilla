//! The per-unit collision height ([`CollisionHeight`]), the reference's `CMovement+0xb4`, derived
//! from the unit's display id and stamped on every streamed entity.

use bevy::prelude::*;

use crate::net::{NetEntity, ObjectStore};

use super::Creatures;

/// A unit's collision height in yards, the reference's `CMovement+0xb4`: the `h` whose fractions
/// are the depth lines (swim `0.75·h`, splash `0.4·h`, foam gate `max(2·h, 1.0)`; `0x6030c0`,
/// `0x5fa760`). It is `CreatureModelData.collisionHeight`, the MD20 collision box's Z extent in
/// model units, times `k = max(SCALE_X, CreatureDisplayInfo.creatureModelScale)`: `0x60b270`
/// passes the larger (`0x60b312`) to `0x6174b0`, which stores the product (`0x617501`), and the
/// unit model build `0x5fb9dd` runs it for every unit.
///
/// The `max` is a floor, not a second multiplier: `SCALE_X` already folds the DBC scales in
/// (vmangos `Unit.cpp:9579`) and is the whole render scale (`0x613fc9`); it bites only where a
/// shrink aura or a `display_scale` override sinks `SCALE_X` under the display's column. The
/// display is the native one (`UNIT_FIELD_NATIVEDISPLAYID`, index 132 at `0x60b270`), so a
/// transform keeps the unit's own depth lines. Not the movement capsule,
/// [`crate::player::CAPSULE_HEIGHT`].
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct CollisionHeight(pub(crate) f32);

impl Default for CollisionHeight {
    /// The `CMovement` ctor's default, never `0.0`, at which every depth line collapses and the
    /// unit swims on dry land.
    fn default() -> Self {
        Self(crate::player::DEFAULT_COLLISION_HEIGHT)
    }
}

/// The display the collision height derives from: `UNIT_FIELD_NATIVEDISPLAYID`, else the rendered
/// display when that is absent or `0` (no descriptor block yet, a synthetic mount child).
pub(crate) fn prism_display_id(store: Option<&ObjectStore>, rendered: Option<u32>) -> Option<u32> {
    store
        .and_then(|s| s.0.unit_native_displayid())
        .and_then(|id| u32::try_from(id).ok())
        .filter(|id| *id != 0)
        .or(rendered)
}

/// [`CollisionHeight`]'s one derivation, shared by the first stamp and the live restamp
/// ([`super::live_display::refresh_live_display`]) so the two cannot disagree.
pub(crate) fn collision_height_for(
    creatures: Option<&Creatures>,
    display_id: Option<u32>,
    scale: f32,
) -> CollisionHeight {
    let row = creatures.zip(display_id);
    let raw = row
        .and_then(|(c, id)| c.collision_height(id))
        // A zero column (67 rows: invisible triggers and the like) means no authored box, so it
        // falls back like a missing row.
        .filter(|h| *h > 0.0)
        .unwrap_or(crate::player::DEFAULT_COLLISION_HEIGHT);
    let k = prism_scale(scale, row.and_then(|(c, id)| c.catalog.display_scale(id)));
    CollisionHeight(raw * k)
}

/// `k = max(SCALE_X, CreatureDisplayInfo.creatureModelScale)`; an unresolved display or a `0`
/// column (3 shipped rows) leaves `SCALE_X` alone. Never 0, at which every depth line collapses.
fn prism_scale(scale: f32, display_scale: Option<f32>) -> f32 {
    display_scale
        .map_or(scale, |s| scale.max(s))
        .max(f32::MIN_POSITIVE)
}

/// Stamp each streamed unit's first [`CollisionHeight`], at the ctor default when its display
/// misses the DBCs; only the unstamped are queried, so it heals if `Creatures` loads late.
pub(super) fn stamp_collision_heights(
    mut commands: Commands,
    creatures: Option<Res<Creatures>>,
    units: Query<(Entity, &NetEntity, Option<&ObjectStore>), Without<CollisionHeight>>,
) {
    for (entity, net, store) in &units {
        let display = prism_display_id(store, net.display_id);
        let h = collision_height_for(creatures.as_deref(), display, net.scale);
        commands.entity(entity).insert(h);
    }
}

#[cfg(test)]
mod native_display {
    use super::prism_display_id;
    use crate::net::ObjectStore;
    use benilla_protocol::ObjectFields;

    /// `UNIT_FIELD_NATIVEDISPLAYID`, between DISPLAYID (131) and MOUNTDISPLAYID (133).
    const NATIVE: u16 = 132;

    fn store(pairs: &[(u16, u32)]) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(pairs))
    }

    /// A druid in bear form: rendered display 2281, native still the night elf's 55.
    #[test]
    fn a_shapeshifted_unit_keeps_its_native_display() {
        let shifted = store(&[(NATIVE, 55)]);
        assert_eq!(prism_display_id(Some(&shifted), Some(2281)), Some(55));
    }

    #[test]
    fn an_absent_or_zero_native_falls_back_to_the_rendered_display() {
        assert_eq!(prism_display_id(None, Some(2281)), Some(2281));
        let empty = store(&[]);
        assert_eq!(prism_display_id(Some(&empty), Some(2281)), Some(2281));
        let zeroed = store(&[(NATIVE, 0)]);
        assert_eq!(prism_display_id(Some(&zeroed), Some(2281)), Some(2281));
        assert_eq!(prism_display_id(Some(&empty), None), None);
    }

    #[test]
    fn an_unshifted_unit_is_unaffected() {
        let plain = store(&[(NATIVE, 4945)]);
        assert_eq!(prism_display_id(Some(&plain), Some(4945)), Some(4945));
    }
}

#[cfg(test)]
mod prism {
    use super::prism_scale;

    #[test]
    fn the_display_column_is_a_floor_never_a_multiplier() {
        // The Shore Strider (display 4945): `modelScale` 1.0 × `CDI.scale` 1.75, folded to 1.75.
        assert_eq!(prism_scale(1.75, Some(1.75)), 1.75);
        // Under the column (a `display_scale` override, a shrink aura): held at the display size.
        assert_eq!(prism_scale(1.0, Some(6.0)), 6.0);
        // A growth aura above the column grows it.
        assert_eq!(prism_scale(3.5, Some(1.75)), 3.5);
        // Never a product: 1.75 × 1.75 would apply the display scale twice.
        assert_ne!(prism_scale(1.75, Some(1.75)), 1.75 * 1.75);
    }

    #[test]
    fn an_unresolved_or_zero_column_leaves_scale_x_alone_and_never_collapses() {
        assert_eq!(prism_scale(1.75, None), 1.75);
        assert_eq!(prism_scale(1.75, Some(0.0)), 1.75, "3 shipped rows carry 0");
        assert!(
            prism_scale(0.0, Some(0.0)) > 0.0,
            "a 0 scale is briefly real"
        );
        assert!(prism_scale(0.0, None) > 0.0);
    }
}
