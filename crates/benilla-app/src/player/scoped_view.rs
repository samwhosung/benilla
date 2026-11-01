//! The spyglass zoom, `SPELL_AURA_FAR_SIGHT` (aura 76): a client-local camera override, unlike the
//! server's `SPELL_AURA_BIND_SIGHT` far sight ([`super::view_subject`]), so no packet carries it.
//! For the local player only (`0x5fa6d0`), the aura watcher `0x604d00` routes to `0x5ff350` (add)
//! and `0x612320` (remove), which match each effect's `EffectApplyAuraName` (`SpellRec+0x16c`)
//! against 76 and call `0x50d320(camera, EffectMiscValue[i])`: first person, locked (camera flag
//! `0x8` makes `SetCameraView` return early), with `[camera+0x40] = n × π/180`; removal restores
//! `π/2`, unlocks and selects view 1. The Ornate Spyglass (item 5507, spell 12883) passes 15, a 6×
//! zoom.
//!
//! [`ScopedView`] follows a change of the aura. A create's `PostInit` shows every standing aura
//! (`0x5fbb3c` → `0x5ff130`), which starts the scope, and our own arrival ends it at once and
//! selects view 1 (`0x5deb44`, `0x50d320(0)`); a fresh create runs no field watcher (`0x465d79`),
//! so a scope held across a worldport comes back off, under the same aura. A `/reload`'s new camera
//! holds no scope either, and the reload itself shows no aura. In the reference the display watcher
//! shows it again (`0x604990` → `0x60abe0` → `0x60ada6` → `0x5ff130` → `0x5ff350` → `0x5ff462`), so
//! a shapeshift or morph under a standing aura restarts the scope; that path is not built here.
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
    /// This frame's arrival found the aura on our create: its `PostInit` started the scope and the
    /// arrival ends it (`0x5fbb3c`, `0x5deb44`), which selects view 1 from any view.
    pub(crate) arrival_end: bool,
    /// The aura the last look found (`0x604d00` compares the slots with their old values).
    aura: Option<u32>,
    /// A new camera was built since the last look.
    camera_rebuilt: bool,
}

impl ScopedView {
    /// While a scope is held the camera is locked in first person.
    pub(crate) fn active(&self) -> bool {
        self.zoom.is_some()
    }

    /// A new camera (`0x50c0e0`) holds no scope, and building it shows no aura.
    pub(super) fn new_camera(&mut self) {
        self.zoom = None;
        self.camera_rebuilt = true;
    }

    /// One look at the aura, `(spell, degrees)`, and whether our create arrived since the last.
    fn observe(&mut self, aura: Option<(u32, f32)>, arrived: bool) {
        let spell = aura.map(|(spell, _)| spell);
        let rebuilt = std::mem::take(&mut self.camera_rebuilt);
        self.arrival_end = arrived && aura.is_some();
        if arrived || rebuilt {
            self.zoom = None;
        } else if spell != self.aura {
            self.zoom = aura.map(|(_, degrees)| degrees / REFERENCE_DEFAULT_DEGREES);
        }
        self.aura = spell;
    }
}

/// Drives the projection from aura 76 in our own aura slots only, as the reference gates it: the
/// aura field is public, so another player's spyglass must not zoom ours.
pub(super) fn apply_scoped_view(
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    arrived: Query<(), Added<SelfPlayer>>,
    spells: Option<Res<Spells>>,
    mut scoped: ResMut<ScopedView>,
    mut fov_zoom: ResMut<FovZoom>,
) {
    let catalog = spells.as_ref().map(|s| &s.catalog);
    let aura = self_q
        .single()
        .ok()
        .zip(catalog)
        .and_then(|(store, catalog)| {
            store.0.unit_auras().find_map(|slot| {
                let rec = catalog.get(slot.spell_id)?;
                (0..3).find_map(|i| {
                    // The reference reads the misc value from the aura name's own effect index.
                    (rec.effect_apply_aura[i] == SPELL_AURA_FAR_SIGHT)
                        .then(|| (slot.spell_id, rec.effect_misc_value[i] as f32))
                })
            })
        })
        // A zero misc value is the reference's own "restore" argument, not a zero-width view.
        .filter(|&(_, deg)| deg > 0.0);
    scoped.observe(aura, !arrived.is_empty());

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
            ..Default::default()
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

        let none = ScopedView::default();
        assert_eq!(
            none.zoom.unwrap_or(1.0),
            1.0,
            "unscoped restores the reference's 90, never a stored default"
        );
        assert!(!none.active());
    }

    const SPYGLASS: u32 = 12883;
    const HELD: Option<(u32, f32)> = Some((SPYGLASS, 15.0));

    /// The aura's add starts the scope and its removal ends it (`0x5ff462`, `0x61236e`).
    #[test]
    fn the_scope_moves_on_a_change_of_the_aura() {
        let mut scoped = ScopedView::default();
        scoped.observe(HELD, false);
        assert_eq!(scoped.zoom, Some(15.0 / REFERENCE_DEFAULT_DEGREES));
        scoped.observe(HELD, false);
        assert!(scoped.active());
        scoped.observe(None, false);
        assert!(!scoped.active());
    }

    /// The create's `PostInit` starts the scope (`0x5fbb3c`) and the arrival ends it (`0x5deb44`),
    /// and here the standing aura starts nothing until it changes (`0x604d00`).
    #[test]
    fn an_arrival_ends_the_scope_under_a_standing_aura() {
        let mut scoped = ScopedView::default();
        scoped.observe(HELD, false);
        scoped.observe(HELD, true);
        assert!(!scoped.active());
        assert!(scoped.arrival_end);
        scoped.observe(HELD, false);
        assert!(!scoped.active());
        assert!(!scoped.arrival_end);
        scoped.observe(None, false);
        scoped.observe(HELD, false);
        assert!(scoped.active(), "a new add starts it");
    }

    /// With no aura on the create nothing started, so no view is owed at this arrival.
    #[test]
    fn an_arrival_with_no_aura_owes_no_view() {
        let mut scoped = ScopedView::default();
        scoped.observe(None, true);
        assert!(!scoped.arrival_end);
    }

    /// A new camera holds no scope, building it shows no aura, and it selects no view.
    #[test]
    fn a_new_camera_ends_the_scope_under_a_standing_aura() {
        let mut scoped = ScopedView::default();
        scoped.observe(HELD, false);
        scoped.new_camera();
        assert!(!scoped.active());
        scoped.observe(HELD, false);
        assert!(!scoped.active());
        assert!(!scoped.arrival_end);
    }

    fn store(fields: &[(u16, u32)]) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(fields))
    }

    /// Our create under the aura holds no scope; the aura's later add does, and zooms.
    #[test]
    fn a_create_under_the_aura_holds_no_scope_and_a_later_add_does() {
        use benilla_formats::{SpellCatalog, SpellDisplay};
        // `UNIT_FIELD_AURA` slot 0 and its `AURAFLAGS` nibble.
        let aura = [(47, SPYGLASS), (95, 8)];
        let mut world = World::new();
        world.init_resource::<ScopedView>();
        world.init_resource::<FovZoom>();
        world.insert_resource(Spells {
            catalog: SpellCatalog::from_displays(std::collections::HashMap::from([(
                SPYGLASS,
                SpellDisplay {
                    effect_apply_aura: [SPELL_AURA_FAR_SIGHT, 0, 0],
                    effect_misc_value: [15, 0, 0],
                    ..Default::default()
                },
            )])),
            ..Spells::empty_for_tests()
        });
        let apply = world.register_system(apply_scoped_view);
        let me = world.spawn((store(&aura), SelfPlayer)).id();
        world.run_system(apply).expect("the look runs");
        assert!(world.resource::<ScopedView>().arrival_end);
        assert!(!world.resource::<ScopedView>().active());
        world.run_system(apply).expect("the look runs");
        assert!(!world.resource::<ScopedView>().arrival_end);
        assert!(!world.resource::<ScopedView>().active());
        world.entity_mut(me).insert(store(&[]));
        world.run_system(apply).expect("the look runs");
        world.entity_mut(me).insert(store(&aura));
        world.run_system(apply).expect("the look runs");
        assert!(world.resource::<ScopedView>().active());
        let zoom = world.resource::<FovZoom>().0;
        assert_eq!(zoom, 15.0 / REFERENCE_DEFAULT_DEGREES);
    }
}
