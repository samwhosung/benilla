//! The per-unit **collision height** ([`CollisionHeight`]) — the reference's `CMovement+0xb4`,
//! derived from the unit's display id and stamped on every streamed entity (decision 0645).
//!
//! It lives beside the display cache because that is where a display id becomes model facts; every
//! *consumer* is elsewhere (`player::swim`, `sound::water`, `sound::footsteps`, `water_fx`,
//! `net::motion::spline`), which is exactly why it is a component and not five private constants.

use bevy::prelude::*;

use crate::net::{NetEntity, ObjectStore};

use super::Creatures;

/// A unit's **collision height** in world yards — the reference's `CMovement+0xb4`, and the `h`
/// that every depth line in the client is a fraction of: swim enters and rests at `0.75·h`, the
/// splash fires at `0.4·h`, wading ends where swimming begins (`0.75·h`), and the foam gate is
/// `max(2·h, 1.0)`. Each of those *fractions* is the real client's (`0x6030c0`, `0x5fa760`);
/// until decision 0645 the `h` they multiplied was a **constant** —
/// [`crate::player::DEFAULT_COLLISION_HEIGHT`], the client's own empty-world ctor default — so all
/// five lines were right for a human male (2.031, within 2 mm of it) and wrong for every other
/// race. A gnome female stands 1.15 yd and was held 1.52 yd under: she could not reach the
/// surface, which is how the defect finally showed (B76).
///
/// `CreatureModelData.collisionHeight × k`. The column is the model's own MD20 collision-box Z
/// extent in raw model units — machine-pinned against the shipped client (`benilla_formats`
/// `collision_height_is_the_m2_collision_box`), which is what settles that it scales with the
/// geometry it bounds.
///
/// **`k = max(OBJECT_FIELD_SCALE_X, CreatureDisplayInfo.creatureModelScale)`** — the byte law:
/// `0x60b270` fetches `SCALE_X` through vtable slot `+0x1c` (`0x469f10`), compares it against the
/// display row's own column at `0x60b312` (`CreatureDisplayInfo+0x10`), and hands the **larger** to
/// `0x6174b0`, which stores `CollisionHeight · k` into `CMovement+0xb4` at `0x617501`. Its two
/// callers include the unit model (re)build `0x5fb9dd` with `force = 1`, so every unit gets it,
/// not just the mover.
///
/// The `max` is a **floor, never a second multiplier**. The *render* scale is [`NetEntity::scale`]
/// = `OBJECT_FIELD_SCALE_X` alone, which the server has already folded the DBC scales into
/// (`CreatureModelData.modelScale × CreatureDisplayInfo.scale`, vmangos `Unit::GetScaleForDisplayId`
/// — and the reference's render scale (`0x613fc9`) has no display-column term, so a client
/// multiplying the display column in again would *square* it). So the collision box and the drawn
/// body still cannot disagree; the floor only stops the box shrinking below the display's own size.
///
/// **At rest the floor is inert, which is why its absence hid**: not one of the 430 shipped
/// `CreatureModelData` rows scales below 1.0, so the folded `SCALE_X ≥ CreatureDisplayInfo.scale`
/// always and `max` picks `SCALE_X`. It bites exactly where `SCALE_X` drops *below* the display's
/// column — a shrink aura, and the **390** `creature_template` rows on our own server whose
/// `display_scale` override undercuts it (by up to 6×). Those units swam at a fraction of their
/// real depth. The floor can only ever *raise* `h`, so no unit that swims correctly today stops.
///
/// **The display it is derived from is the unit's NATIVE one** — `UNIT_FIELD_NATIVEDISPLAYID`, not
/// the one being rendered. `0x60b270` fetches its `CreatureDisplayInfo` row at
/// `[unit+0x110]+0x1f8` (index 132), while its sibling `0x60ae10` reads `+0x1f4` (index 131 =
/// `UNIT_FIELD_DISPLAYID`) off the same array; the cached-row arm is taken only when the two
/// fields are equal (`[unit+0xc58] & 0x100` = "not transformed", set from their equality at
/// `0x60b166`), so **every arm yields the native model's row**.
///
/// So **a transform does not resize the collision prism**: a druid in bear form keeps the druid's
/// swim, splash, foam and wade lines; a GM `.modify morph` into a gnome keeps the human's. That is
/// the reference deliberately letting the collision box and the drawn body disagree — and nothing
/// here needs them to agree, because every consumer of this number is a *depth line* (a fraction of
/// `h`), never a rendered size. Decision 0695 restamped it from the rendered display on the
/// opposite premise ("the collision box and the drawn body can never disagree"); 1574 corrects that.
///
/// **Not** the movement capsule: that stays the constant-height tunable it has always been
/// ([`crate::player::CAPSULE_HEIGHT`] — see its doc for why the two are deliberately separate).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct CollisionHeight(pub(crate) f32);

impl Default for CollisionHeight {
    /// The client's own ctor default — never `0.0`, at which every depth line collapses and the
    /// unit swims on dry land. This is what makes the type safe to hold in a `Default`-derived
    /// struct ([`crate::player::Player`] does).
    fn default() -> Self {
        Self(crate::player::DEFAULT_COLLISION_HEIGHT)
    }
}

/// The display id the collision prism is derived from: **`UNIT_FIELD_NATIVEDISPLAYID`**, falling
/// back to the rendered display when the field is absent (a unit with no descriptor block yet, a
/// synthetic entity like a mount child). `0` is treated as absent — the wire's own "unset".
///
/// This is the whole of decision 1574: one field swap, at the two sites that derive the prism.
pub(crate) fn prism_display_id(store: Option<&ObjectStore>, rendered: Option<u32>) -> Option<u32> {
    store
        .and_then(|s| s.0.unit_native_displayid())
        .and_then(|id| u32::try_from(id).ok())
        .filter(|id| *id != 0)
        .or(rendered)
}

/// A unit's collision height from its display id and render scale — [`CollisionHeight`]'s one
/// derivation, shared by the ECS stamp below and the avatar's mirror of it
/// (`player::wire_in`), so the two can never drift into different answers.
pub(crate) fn collision_height_for(
    creatures: Option<&Creatures>,
    display_id: Option<u32>,
    scale: f32,
) -> CollisionHeight {
    let row = creatures.zip(display_id);
    let raw = row
        .and_then(|(c, id)| c.collision_height(id))
        // A zero column is real in the data (67 rows — invisible triggers and the like); it means
        // "no authored box", not "a unit of no height", so it falls back like a missing row.
        .filter(|h| *h > 0.0)
        .unwrap_or(crate::player::DEFAULT_COLLISION_HEIGHT);
    let k = prism_scale(scale, row.and_then(|(c, id)| c.catalog.display_scale(id)));
    CollisionHeight(raw * k)
}

/// `k = max(OBJECT_FIELD_SCALE_X, CreatureDisplayInfo.creatureModelScale)` — the collision prism's
/// multiplier, split out so the FLOOR is assertable on its own (see [`CollisionHeight`] for the
/// byte law and why the floor is not a second multiplier). A display we can't resolve — and the
/// three shipped rows whose column is 0 — contributes nothing and leaves `SCALE_X` standing alone.
/// Never returns 0: at zero every depth line collapses and the unit swims on dry land.
fn prism_scale(scale: f32, display_scale: Option<f32>) -> f32 {
    display_scale
        .map_or(scale, |s| scale.max(s))
        .max(f32::MIN_POSITIVE)
}

/// Give every streamed unit its [`CollisionHeight`] the frame its display resolves. Queries the
/// not-yet-stamped only, so it costs nothing in steady state and self-heals if `Creatures` (the DBC
/// catalog) lands after the first units stream in. A display id that misses the DBCs — or a unit
/// carrying none at all — is still stamped, at the ctor default, so no consumer has to tell
/// "not resolved yet" apart from "resolved to nothing".
///
/// This system stamps only the **first** value. A live change to either input — the unit's
/// **native** display id or its `SCALE_X` — restamps through
/// [`super::live_display::refresh_live_display`]. A *rendered* display swap (druid form, GM morph)
/// deliberately does **not**: see [`CollisionHeight`] for why the reference lets the collision box
/// and the drawn body disagree.
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

    /// `UNIT_FIELD_NATIVEDISPLAYID`, index 132 — the gap between DISPLAYID (131) and
    /// MOUNTDISPLAYID (133), both of which this crate already pins independently.
    const NATIVE: u16 = 132;

    fn store(pairs: &[(u16, u32)]) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(pairs))
    }

    /// A druid in bear form: rendered display 2281, native still the night elf's 55. The prism
    /// follows the native one — decision 1574's whole observable.
    #[test]
    fn a_shapeshifted_unit_keeps_its_native_display() {
        let shifted = store(&[(NATIVE, 55)]);
        assert_eq!(prism_display_id(Some(&shifted), Some(2281)), Some(55));
    }

    /// The fallbacks, all three of them, all landing on the rendered display: no descriptor block
    /// yet, a descriptor without the field, and the wire's own "unset" zero.
    #[test]
    fn an_absent_or_zero_native_falls_back_to_the_rendered_display() {
        assert_eq!(prism_display_id(None, Some(2281)), Some(2281));
        let empty = store(&[]);
        assert_eq!(prism_display_id(Some(&empty), Some(2281)), Some(2281));
        let zeroed = store(&[(NATIVE, 0)]);
        assert_eq!(prism_display_id(Some(&zeroed), Some(2281)), Some(2281));
        // …and a unit with neither stays None, which `collision_height_for` reads as "no row".
        assert_eq!(prism_display_id(Some(&empty), None), None);
    }

    /// An unshifted unit — native == rendered — is the common case and must be untouched.
    #[test]
    fn an_unshifted_unit_is_unaffected() {
        let plain = store(&[(NATIVE, 4945)]);
        assert_eq!(prism_display_id(Some(&plain), Some(4945)), Some(4945));
    }
}

#[cfg(test)]
mod prism {
    use super::prism_scale;

    /// The **floor**, and the two shapes it has to get right. `SCALE_X` normally already carries
    /// the DBC product (the server folded it), so the display column can only ever equal or
    /// undercut it — which is why the floor is inert on shipped data and bit only when a server
    /// override or a shrink aura pushed `SCALE_X` under the display's own size.
    #[test]
    fn the_display_column_is_a_floor_never_a_multiplier() {
        // The Shore Strider (display 4945): `modelScale` 1.0 × `CDI.scale` 1.75, folded → 1.75.
        // The floor is inert here, which is exactly why the height was NOT B311's cause.
        assert_eq!(prism_scale(1.75, Some(1.75)), 1.75);
        // A `creature_template.display_scale` override under the display's own column (390 rows on
        // our server), or a shrink aura: the prism holds at the display size, it does not shrink.
        assert_eq!(prism_scale(1.0, Some(6.0)), 6.0);
        // A growth aura pushes SCALE_X above the column: the prism grows with it.
        assert_eq!(prism_scale(3.5, Some(1.75)), 3.5);
        // …and it is never a product: 1.75 × 1.75 = 3.06 would be the double-apply.
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
