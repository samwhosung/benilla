//! The cursor while a cast waits for its click: the ground point's range verdict
//! (`CheckGroundPointInRange 0x6e6810`, in `0x4820f0`), the hovered object's validity (`0x6e6460`,
//! in `0x4828d0`) and the reticle's radius (`GetCurrentCastRadius 0x6e6350`).

use bevy::prelude::*;

use benilla_formats::SpellRange;

use crate::net::SelfPlayer;
use crate::target::{CursorKind, PickOcclusion, WorldCursor};
use crate::ui_action::Spells;

use super::{SpellTargeting, TargetingWants};

/// `CheckGroundPointInRange 0x6e6810`: min² and max² from the `SpellRange` row against the squared
/// caster-to-point distance. Its one caller is the hover classifier `0x4820f0`, so it colours the
/// cursor and the click never asks. No row is permissive; the server judges every send.
fn ground_point_in_range(row: Option<&SpellRange>, self_pos: Vec3, point: Vec3) -> bool {
    let Some(row) = row else {
        return true;
    };
    let dist_sq = self_pos.distance_squared(point);
    if row.min > 0.0 && dist_sq < row.min * row.min {
        return false;
    }
    dist_sq <= row.max * row.max
}

fn range_row(spells: Option<&Spells>, spell_id: u32) -> Option<&SpellRange> {
    let spells = spells?;
    spells.ranges.get(spells.catalog.get(spell_id)?.range_index)
}

/// `GetCurrentCastRadius 0x6e6350`: per effect `radius + casterLevel × perLevel` over
/// `EffectRadiusIndex[0]` and `[1]` only, the larger with slot 1 winning ties and NaN, clamped to
/// 20.0 (`0x4820f0`'s `[0x804478]`). 0.0 means no radius rows and the reticle's default size.
/// The reference then applies spell-mod op 6 (SPELLMOD_RADIUS, `0x6e6bf0`); this does not yet,
/// though `crate::spell::mods` has it.
pub(crate) fn ground_cast_radius(spells: Option<&Spells>, spell_id: u32, level: u32) -> f32 {
    let Some(spells) = spells else { return 0.0 };
    let Some(d) = spells.catalog.get(spell_id) else {
        return 0.0;
    };
    let candidate = |slot: usize| -> f32 {
        let idx = d.effect_radius_index[slot];
        if idx == 0 {
            return 0.0;
        }
        spells
            .radii
            .get(idx)
            .map_or(0.0, |r| r.radius + level as f32 * r.per_level)
    };
    let (c0, c1) = (candidate(0), candidate(1));
    // Strict > for slot 0: a tie or a NaN falls to slot 1, as the reference compares.
    let r = if c0 > c1 { c0 } else { c1 };
    r.min(20.0)
}

/// While targeting, runs after the world classifier and overwrites its verdict (`0x4820f0`'s
/// pre-empt). It runs every frame, even over a UI frame, because the reticle reads
/// `WorldCursor.unable` as its colour; the reference's hover gate `0x481790` (WorldFrame has mouse
/// focus) lives in [`crate::cursor`].
///
/// The pick flags come from the word alone (`0x481050`), so the word picks the handler:
///
/// | pick state | handler | verdict |
/// |---|---|---|
/// | 1, terrain | `0x4820f0` | `CheckGroundPointInRange 0x6e6810` on the ground point |
/// | 2, object | `0x4828d0` → `0x6e6460` | the hovered object's validity |
/// | 0, nothing | `0x481790`'s tail | `CursorSetMode(0x16)`, UnableCast |
///
/// A word without `& 0x60` sets no terrain pick bit, so an armed lockpick is grey except over a
/// GameObject it can open. An item-only word (`0x0010`) sets no pick flags, the pick bails before
/// its ray (`0x4812c8`) and the world cursor stays grey.
///
/// The object arm is `0x6e6460`'s GameObject leg: `word & 0x4800`, the lock predicate `0x5f8260`,
/// then the same min/max range test through `GetMinMaxRange 0x6e3480`. The unit leg runs the
/// standing word through the cast arm's shared unit binder. World-item and Corpse legs remain
/// unmodeled.
///
/// Every seam shows the `Cast` kind, so this reads the whole-word [`SpellTargeting::spell`].
pub(crate) fn drive_targeting_cursor(
    targeting: Res<SpellTargeting>,
    unit_checks: super::UnitBindChecks,
    occlusion: Res<PickOcclusion>,
    hovered: Res<crate::target::Hovered>,
    hovered_object: Res<crate::target::HoveredObject>,
    spells: Option<Res<Spells>>,
    self_tf: Query<&Transform, With<SelfPlayer>>,
    go_tf: Query<&Transform>,
    stores: Query<(
        &crate::net::ObjectStore,
        Option<&crate::go_anim::GoAnim>,
        &Transform,
    )>,
    // Read-only: the template request is made at stream-in (`net::objects`), so a cold cache
    // greys one frame, not forever.
    lock_inputs: crate::target::lock::GoLockInputs,
    mut cursor: ResMut<WorldCursor>,
) {
    let Some(spell_id) = targeting.spell() else {
        return;
    };
    let row = range_row(spells.as_deref(), spell_id);
    let me = self_tf.single().ok().map(|tf| tf.translation);
    // "A GameObject is the nearest pick" is the same test the click uses
    // ([`super::world::commit_object_cast_on_click`]).
    let able = if targeting.wants(TargetingWants::Unit)
        && hovered
            .target
            .is_some_and(|entity| targeting.can_bind_unit(entity, &unit_checks))
    {
        true
    } else if targeting.wants(TargetingWants::GameObject)
        && crate::target::go_is_nearest(&hovered, &hovered_object)
    {
        object_arm(
            &hovered_object,
            &stores,
            &go_tf,
            &lock_inputs,
            spell_id,
            row,
            me,
        )
    } else if targeting.wants(TargetingWants::Location) {
        // `0x4820f0`. No ground hit (sky, mouselook) is state 0, UnableCast.
        match (occlusion.point, me) {
            (Some(point), Some(me)) => ground_point_in_range(row, me, point),
            _ => false,
        }
    } else {
        // Pick state 0: nothing this word can bind is under the cursor.
        false
    };
    *cursor = WorldCursor {
        kind: CursorKind::Cast,
        unable: !able,
    };
}

/// `0x6e6460`'s GameObject leg past the `& 0x4800` test: the lock predicate `0x5f8260`, then range.
fn object_arm(
    hovered_object: &crate::target::HoveredObject,
    stores: &Query<(
        &crate::net::ObjectStore,
        Option<&crate::go_anim::GoAnim>,
        &Transform,
    )>,
    go_tf: &Query<&Transform>,
    lock_inputs: &crate::target::lock::GoLockInputs,
    spell_id: u32,
    row: Option<&SpellRange>,
    me: Option<Vec3>,
) -> bool {
    let (Some(entity), Some(guid)) = (hovered_object.target, hovered_object.guid) else {
        return false;
    };
    // `0x5f8260`'s lookups: the template, then its `Lock.dbc` row. A template in flight is grey.
    let Some(tmpl) = lock_inputs.templates.get(guid) else {
        return false;
    };
    let lock_id = tmpl.lock_id;
    let Some(locks) = lock_inputs.locks.as_deref() else {
        return false;
    };
    let Some(slots) = locks.0.slots(lock_id).filter(|_| lock_id != 0) else {
        return false;
    };
    let Some(spells) = lock_inputs.spells.as_deref() else {
        return false;
    };
    let Some(spell) = spells.catalog.get(spell_id) else {
        return false;
    };
    let facts = crate::target::lock::go_facts(
        stores
            .get(entity)
            .ok()
            .map(|(s, anim, _)| (s, crate::go_anim::go_state(anim, s))),
    );
    if !crate::target::lock::spell_opens_lock(slots, spell, facts) {
        return false;
    }
    // The range tail: the caster-to-object distance against the spell's min and max.
    match (me, go_tf.get(entity)) {
        (Some(me), Ok(tf)) => ground_point_in_range(row, me, tf.translation),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blizzard's row 4 is 0 to 30 yd; a synthetic min exercises the too-close arm.
    #[test]
    fn ground_point_in_range_mirrors_check_ground_point_in_range() {
        let row = |min: f32, max: f32| SpellRange { min, max, flags: 0 };
        let origin = Vec3::ZERO;
        let at = |d: f32| Vec3::new(d, 0.0, 0.0);
        let blizzard = row(0.0, 30.0);
        assert!(ground_point_in_range(Some(&blizzard), origin, at(29.9)));
        assert!(!ground_point_in_range(Some(&blizzard), origin, at(30.1)));
        let banded = row(8.0, 35.0);
        assert!(!ground_point_in_range(Some(&banded), origin, at(5.0)));
        assert!(ground_point_in_range(Some(&banded), origin, at(20.0)));
        // No row → permissive (the server still validates).
        assert!(ground_point_in_range(None, origin, at(500.0)));
    }

    /// Fixture rows mirror the real table: row 14 is 8.0 (Blizzard), row 8 is 5.0 (Flamestrike).
    #[test]
    fn ground_cast_radius_mirrors_get_current_cast_radius() {
        use benilla_formats::{SpellDisplay, SpellRadius};
        use std::collections::HashMap;
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        let display = |idx: [u32; 3]| SpellDisplay {
            effect_radius_index: idx,
            ..SpellDisplay::default()
        };
        spells.catalog = benilla_formats::SpellCatalog::from_displays(HashMap::from([
            (10, display([14, 0, 0])),
            (2120, display([8, 8, 0])),
            (777, display([0, 0, 13])), // slot 2 only, never read
            (778, display([90, 8, 0])), // per-level row in slot 0
            (779, display([10, 0, 0])), // row 10 = 30.0, over the 20.0 clamp
        ]));
        spells.radii = benilla_formats::SpellRadiusCatalog::from_rows(HashMap::from([
            (
                14,
                SpellRadius {
                    radius: 8.0,
                    per_level: 0.0,
                    max: 0.0,
                },
            ),
            (
                8,
                SpellRadius {
                    radius: 5.0,
                    per_level: 0.0,
                    max: 0.0,
                },
            ),
            (
                13,
                SpellRadius {
                    radius: 10.0,
                    per_level: 0.0,
                    max: 0.0,
                },
            ),
            (
                10,
                SpellRadius {
                    radius: 30.0,
                    per_level: 0.0,
                    max: 0.0,
                },
            ),
            (
                90,
                SpellRadius {
                    radius: 2.0,
                    per_level: 0.1,
                    max: 0.0,
                },
            ),
        ]));
        let s = Some(&spells);
        assert_eq!(ground_cast_radius(s, 10, 60), 8.0);
        assert_eq!(ground_cast_radius(s, 2120, 60), 5.0);
        // Slot 2 is never read: no rows in slots 0 and 1 reads 0, the default size.
        assert_eq!(ground_cast_radius(s, 777, 60), 0.0);
        // Per-level: 2.0 + 60 × 0.1 = 8.0 beats slot 1's 5.0.
        assert_eq!(ground_cast_radius(s, 778, 60), 8.0);
        // The 20.0 clamp (`[0x804478]`).
        assert_eq!(ground_cast_radius(s, 779, 60), 20.0);
        // Unknown spell or no data: 0, the default size.
        assert_eq!(ground_cast_radius(s, 9999, 60), 0.0);
        assert_eq!(ground_cast_radius(None, 10, 60), 0.0);
    }

    /// Only two of the three pick states are handlers; state 0 is UnableCast.
    #[test]
    fn the_cursor_is_grey_wherever_the_word_has_no_handler() {
        use bevy::ecs::system::RunSystemOnce;

        let verdict = |word: u16, point: Option<Vec3>, go: Option<f32>| {
            let mut world = World::new();
            world.init_resource::<WorldCursor>();
            world.init_resource::<SpellTargeting>();
            world.init_resource::<crate::target::Hovered>();
            world.init_resource::<crate::target::HoveredObject>();
            world.init_resource::<crate::go_templates::GameObjectTemplates>();
            world.init_resource::<crate::items::Items>();
            world.init_resource::<crate::net::GuidIndex>();
            world.insert_resource(PickOcclusion {
                distance: 10.0,
                point,
            });
            if let Some(distance) = go {
                let chest = world.spawn(Transform::default()).id();
                world.insert_resource(crate::target::HoveredObject {
                    target: Some(chest),
                    guid: Some(0x1234),
                    distance,
                });
            }
            world.spawn((SelfPlayer, Transform::default()));
            world.resource_mut::<SpellTargeting>().enter(
                2120,
                crate::spell::CastCommit::Spell,
                word,
            );
            world
                .run_system_once(drive_targeting_cursor)
                .expect("the targeting cursor drives");
            let cursor = world.resource::<WorldCursor>();
            assert_eq!(cursor.kind, CursorKind::Cast, "the KIND is always Cast");
            !cursor.unable
        };

        // Pick state 1: Blizzard's DEST word over a ground point. No `Spells` resource means no
        // range row, which is permissive.
        assert!(
            verdict(0x0040, Some(Vec3::ZERO), None),
            "ground point → Cast"
        );
        // …and over sky / mouselook there is no point: state 0.
        assert!(!verdict(0x0040, None, None), "no ground hit → UnableCast");

        // An item-only word (`0x0010`): no pick flags, the pick bails at `0x4812c8`, always grey.
        assert!(!verdict(0x0010, Some(Vec3::ZERO), None));
        assert!(!verdict(0x0010, None, None));
        // Even over a GameObject: `& 0x4800` is 0.
        assert!(!verdict(0x0010, Some(Vec3::ZERO), Some(3.0)));

        // A GameObject word (Opening, `0x4800`) over bare ground: state 0.
        assert!(
            !verdict(0x4800, Some(Vec3::ZERO), None),
            "lockpick over dirt is grey"
        );
        // Over a GameObject whose template has not streamed in: the object arm bails to grey.
        assert!(!verdict(0x4800, Some(Vec3::ZERO), Some(3.0)));

        // A lock word that also carries DEST (`0x4840`) keeps its terrain handler off a GameObject.
        assert!(verdict(0x4840, Some(Vec3::ZERO), None));
    }
}
