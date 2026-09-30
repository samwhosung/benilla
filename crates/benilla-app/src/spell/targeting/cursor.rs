//! The cursor while a cast waits for its click: the ground point's range verdict
//! (`CheckGroundPointInRange 0x6e6810`, in `0x4820f0`), the hovered object's validity (`0x6e6460`,
//! in `0x4828d0`: a GameObject, a unit or a corpse) and the reticle's radius
//! (`GetCurrentCastRadius 0x6e6350`).

use bevy::prelude::*;

use benilla_formats::{RangeTargets, RangeUnit, SpellDisplay, SpellRange};

use crate::net::SelfPlayer;
use crate::spell::SpellModifiers;
use crate::target::{CursorKind, PickOcclusion, WorldCursor};
use crate::ui_action::Spells;

use super::{SpellTargeting, TargetingWants};

/// What `GetMinMaxRange 0x6e3480` reads for the cursor's two range verdicts: the ground point
/// (`CheckGroundPointInRange 0x6e6810`, a null target at `0x6e6879`) and the GameObject leg of
/// `0x6e6460` (the object as target at `0x6e679b`). Neither passes a unit, which the function
/// tests for at `0x6e34e1`-`0x6e34f9`: the ranged arm pads nothing and takes no moving bonus,
/// and the melee arm sums the caster's reach with its auto-attack target's and asks that unit's
/// motion, with the caster's, for the bonus.
#[derive(Clone, Copy)]
struct RangeCall<'a> {
    spell: &'a SpellDisplay,
    row: &'a SpellRange,
    mods: &'a SpellModifiers,
    caster: RangeUnit,
    attack_target: Option<RangeUnit>,
}

/// The call's inputs, or `None` where the data is absent (no catalog, an unknown spell, a row
/// missing from `SpellRange`), which is permissive: the server judges every send.
fn range_call<'a>(checks: &'a super::BindChecks, spell_id: u32) -> Option<RangeCall<'a>> {
    let spells = checks.spells.as_deref()?;
    let spell = spells.catalog.get(spell_id)?;
    Some(RangeCall {
        spell,
        row: spells.ranges.get(spell.range_index)?,
        mods: &checks.spell_mods,
        caster: checks.range_units.caster(),
        attack_target: checks.range_units.caster_attack_target(),
    })
}

/// `CheckGroundPointInRange 0x6e6810`: the squared caster-to-point distance against
/// `GetMinMaxRange`'s bounds, whose max carries spell-mod op 5 (`0x6e3744`), so a range talent
/// moves the verdict. The compare fails on min² > d² (`0x6e68b5`-`0x6e68c3`) and on max² < d²
/// (`0x6e68c5`-`0x6e68d3`), a NaN failing both; the min has no `> 0` guard, a zero one is inert.
/// Its one caller is the hover classifier `0x4820f0`, so it colours the cursor and the click never
/// asks.
fn ground_point_in_range(call: Option<RangeCall>, self_pos: Vec3, point: Vec3) -> bool {
    let Some(call) = call else {
        return true;
    };
    let targets = RangeTargets {
        target: None,
        attack_target: call.attack_target,
    };
    // The self row is `{0, 0}` (`0x6e35dd`, `0x6e35ea`) and `min_max_range` answers `None` for it:
    // only the caster's own spot passes it here.
    let (min, max) = call
        .mods
        .min_max_range(call.spell, Some(call.row), call.caster, targets)
        .unwrap_or_default();
    let dist_sq = self_pos.distance_squared(point);
    min * min <= dist_sq && dist_sq <= max * max
}

/// `GetCurrentCastRadius 0x6e6350`: per effect `radius + casterLevel × perLevel` over
/// `EffectRadiusIndex[0]` and `[1]` only, the larger with slot 1 winning ties and NaN, clamped to
/// 20.0 (`0x4820f0`'s `[0x804478]`). 0.0 means no radius rows and the reticle's default size.
/// The reference then applies spell-mod op 6 (SPELLMOD_RADIUS, `0x6e6bf0`) before the reticle's
/// 20-yard clamp (`0x4820f0`).
pub(crate) fn ground_cast_radius(
    spells: Option<&Spells>,
    spell_id: u32,
    level: u32,
    mods: &super::super::SpellModifiers,
) -> f32 {
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
    mods.apply_float(d, super::super::OP_RADIUS, r).min(20.0)
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
/// then the same min/max range test through `GetMinMaxRange 0x6e3480`, its range modifier included.
/// Its unit leg is
/// [`SpellTargeting::can_target_unit`] and its corpse leg [`SpellTargeting::can_target_corpse`],
/// over a corpse the pick admitted under a word in `0x8600`. The world-item leg is not built.
///
/// Every seam shows the `Cast` kind, so this reads the whole-word [`SpellTargeting::spell`].
pub(crate) fn drive_targeting_cursor(
    targeting: Res<SpellTargeting>,
    checks: super::BindChecks,
    occlusion: Res<PickOcclusion>,
    hovered: Res<crate::target::Hovered>,
    hovered_object: Res<crate::target::HoveredObject>,
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
    let range = range_call(&checks, spell_id);
    let me = self_tf.single().ok().map(|tf| tf.translation);
    // The arm the click would take ([`super::world::commit_object_cast_on_click`]): a GameObject
    // word's nearest GameObject, else the picked unit or corpse, else the terrain.
    let able = if targeting.wants(TargetingWants::GameObject)
        && crate::target::go_is_nearest(&hovered, &hovered_object)
    {
        object_arm(
            &hovered_object,
            &stores,
            &go_tf,
            &lock_inputs,
            spell_id,
            range,
            me,
        )
    } else if let Some(unit) = hovered
        .target
        .filter(|_| targeting.wants(TargetingWants::Unit))
    {
        targeting.can_target_unit(unit, &checks)
    } else if let Some(corpse) = hovered
        .corpse
        .filter(|_| targeting.wants(TargetingWants::Corpse))
    {
        targeting.can_target_corpse(corpse, &checks)
    } else if targeting.wants(TargetingWants::Location) {
        // `0x4820f0`. No ground hit (sky, mouselook) is state 0, UnableCast.
        match (occlusion.point, me) {
            (Some(point), Some(me)) => ground_point_in_range(range, me, point),
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
    range: Option<RangeCall>,
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
        (Some(me), Ok(tf)) => ground_point_in_range(range, me, tf.translation),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The call for a spell and row under these tables, with the default combat reach and no
    /// auto-attack target.
    fn call<'a>(
        spell: &'a SpellDisplay,
        row: &'a SpellRange,
        mods: &'a SpellModifiers,
    ) -> Option<RangeCall<'a>> {
        Some(RangeCall {
            spell,
            row,
            mods,
            caster: RangeUnit::still(1.5),
            attack_target: None,
        })
    }

    /// Blizzard's row 4 is 0 to 30 yd; a synthetic min exercises the too-close arm.
    #[test]
    fn ground_point_in_range_mirrors_check_ground_point_in_range() {
        let row = |min: f32, max: f32| SpellRange { min, max, flags: 0 };
        let origin = Vec3::ZERO;
        let at = |d: f32| Vec3::new(d, 0.0, 0.0);
        let spell = SpellDisplay::default();
        let mods = SpellModifiers::default();
        let blizzard = row(0.0, 30.0);
        let ground = |row: &SpellRange, d: f32| {
            ground_point_in_range(call(&spell, row, &mods), origin, at(d))
        };
        assert!(ground(&blizzard, 29.9));
        assert!(!ground(&blizzard, 30.1));
        let banded = row(8.0, 35.0);
        assert!(!ground(&banded, 5.0));
        assert!(ground(&banded, 20.0));
        // Both bounds are inclusive (`min² > d²` and `max² < d²` fail, `0x6e68c3`, `0x6e68d3`), and
        // a NaN distance fails both.
        assert!(ground(&banded, 8.0));
        assert!(ground(&banded, 35.0));
        assert!(!ground(&banded, f32::NAN));
        assert!(!ground(&blizzard, f32::NAN));
        // The self row is `{0, 0}`: only the caster's own spot is in range.
        let own = row(0.0, 0.0);
        assert!(ground(&own, 0.0));
        assert!(!ground(&own, 1.0));
        // No row → permissive (the server still validates).
        assert!(ground_point_in_range(None, origin, at(500.0)));
    }

    /// Arctic Reach adds op 5 with family mask `0xa0` (bits 5 and 7), and Blizzard's `0x80080` sets
    /// bit 7: 30 yd at +20% is 36. The talent moves the max only, and a spell whose mask it does
    /// not cover keeps its 30.
    #[test]
    fn a_range_talent_moves_the_ground_verdict() {
        let mage = |flags: u64| SpellDisplay {
            spell_family: 3,
            spell_family_flags: flags,
            ..SpellDisplay::default()
        };
        let blizzard = mage(0x8_0080);
        let flamestrike = mage(0x4000_0004);
        let row = SpellRange {
            min: 0.0,
            max: 30.0,
            flags: 0,
        };
        let banded = SpellRange {
            min: 8.0,
            max: 35.0,
            flags: 0,
        };
        let plain = {
            let mut mods = SpellModifiers::default();
            mods.set_class_family(3);
            mods
        };
        let mut talented = plain.clone();
        for bit in [5, 7] {
            talented.set(false, bit, crate::spell::OP_RANGE, 20);
        }
        let ground = |spell: &SpellDisplay, row: &SpellRange, mods: &SpellModifiers, d: f32| {
            ground_point_in_range(call(spell, row, mods), Vec3::ZERO, Vec3::new(d, 0.0, 0.0))
        };

        assert!(!ground(&blizzard, &row, &plain, 33.0), "30 yd without it");
        assert!(ground(&blizzard, &row, &talented, 29.9));
        assert!(ground(&blizzard, &row, &talented, 33.0));
        assert!(ground(&blizzard, &row, &talented, 35.9));
        assert!(!ground(&blizzard, &row, &talented, 36.1));
        assert!(!ground(&blizzard, &row, &talented, 37.0));
        // Bit 7 is not in Flamestrike's mask (bits 2 and 30).
        assert!(!ground(&flamestrike, &row, &talented, 33.0));
        // Only the max moves: 8 to 35 is 8 to 42.
        assert!(!ground(&blizzard, &banded, &talented, 5.0));
        assert!(ground(&blizzard, &banded, &talented, 41.0));
        assert!(!ground(&blizzard, &banded, &talented, 43.0));
    }

    /// With no unit passed, the melee arm sums the caster's reach with its auto-attack target's, else
    /// its own, and the max is floored at 5.0 (`0x6e3552`-`0x6e35bf`).
    #[test]
    fn the_melee_row_reads_the_casters_reaches() {
        let melee = SpellRange {
            min: 0.0,
            max: 5.0,
            flags: 1,
        };
        let spell = SpellDisplay::default();
        let mods = SpellModifiers::default();
        let ground = |self_reach: f32, attack_target_reach: Option<f32>, d: f32| {
            let call = RangeCall {
                spell: &spell,
                row: &melee,
                mods: &mods,
                caster: RangeUnit::still(self_reach),
                attack_target: attack_target_reach.map(RangeUnit::still),
            };
            ground_point_in_range(Some(call), Vec3::ZERO, Vec3::new(d, 0.0, 0.0))
        };
        // 1.5 + 1.5 + 1.3333 is under the floor.
        assert!(ground(1.5, None, 4.9));
        assert!(!ground(1.5, None, 5.1));
        // The caster's 3.0 twice: 7.33.
        assert!(ground(3.0, None, 7.2));
        assert!(!ground(3.0, None, 7.5));
        // An auto-attack target's 4.0 in place of the second: 1.5 + 4.0 + 1.3333 = 6.83.
        assert!(ground(1.5, Some(4.0), 6.7));
        assert!(!ground(1.5, Some(4.0), 7.0));
    }

    /// The moving bonus (`0x6e3648`-`0x6e36a2`) on the ground verdict: with no unit passed, only
    /// the melee arm has a unit for it, the auto-attack target, and both must run. The ranged arm
    /// has none, whoever the caster's auto-attack target is.
    #[test]
    fn the_ground_verdict_takes_the_moving_bonus_on_the_melee_row_alone() {
        let running = RangeUnit {
            // A player, which would earn a ranged target the bonus, were one passed.
            player: true,
            motion: benilla_formats::UnitMotion {
                flags: 1,
                speed: 7.0,
                walk_speed: 2.5,
            },
            ..RangeUnit::still(1.5)
        };
        let melee = SpellRange {
            min: 0.0,
            max: 5.0,
            flags: 1,
        };
        let blizzard = SpellRange {
            min: 0.0,
            max: 30.0,
            flags: 0,
        };
        let spell = SpellDisplay::default();
        let mods = SpellModifiers::default();
        let ground = |row: &SpellRange, caster, attack_target, d: f32| {
            let call = RangeCall {
                spell: &spell,
                row,
                mods: &mods,
                caster,
                attack_target,
            };
            ground_point_in_range(Some(call), Vec3::ZERO, Vec3::new(d, 0.0, 0.0))
        };
        // 5.0 floor + 2.6667 = 7.667 for two runners.
        assert!(ground(&melee, running, Some(running), 7.5));
        assert!(!ground(&melee, running, Some(running), 7.8));
        // One of them standing, or no auto-attack target, and the floor stands.
        let still = RangeUnit::still(1.5);
        assert!(!ground(&melee, running, Some(still), 7.5));
        assert!(!ground(&melee, still, Some(running), 7.5));
        assert!(!ground(&melee, running, None, 7.5));
        // The ranged arm never reads the auto-attack target: 30, not 32.67.
        assert!(!ground(&blizzard, running, Some(running), 31.0));
    }

    /// Fixture rows mirror the real table: row 14 is 8.0 (Blizzard), row 8 is 5.0 (Flamestrike).
    #[test]
    fn ground_cast_radius_mirrors_get_current_cast_radius() {
        use benilla_formats::{SpellDisplay, SpellRadius};
        use std::collections::HashMap;
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        let display = |idx: [u32; 3]| SpellDisplay {
            effect_radius_index: idx,
            spell_family: 3,
            spell_family_flags: 1 << 5,
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
        let mods = crate::spell::SpellModifiers::default();
        assert_eq!(ground_cast_radius(s, 10, 60, &mods), 8.0);
        assert_eq!(ground_cast_radius(s, 2120, 60, &mods), 5.0);
        // Slot 2 is never read: no rows in slots 0 and 1 reads 0, the default size.
        assert_eq!(ground_cast_radius(s, 777, 60, &mods), 0.0);
        // Per-level: 2.0 + 60 × 0.1 = 8.0 beats slot 1's 5.0.
        assert_eq!(ground_cast_radius(s, 778, 60, &mods), 8.0);
        // The 20.0 clamp (`[0x804478]`).
        assert_eq!(ground_cast_radius(s, 779, 60, &mods), 20.0);
        // Unknown spell or no data: 0, the default size.
        assert_eq!(ground_cast_radius(s, 9999, 60, &mods), 0.0);
        assert_eq!(ground_cast_radius(None, 10, 60, &mods), 0.0);

        let mut mods = crate::spell::SpellModifiers::default();
        mods.set_class_family(3);
        mods.set(false, 5, crate::spell::OP_RADIUS, -50);
        assert_eq!(ground_cast_radius(s, 779, 60, &mods), 15.0);
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
            world.init_resource::<crate::spell::SpellModifiers>();
            world.insert_resource(crate::net::Reputations(Vec::new()));
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

    /// `0x4820f0` over terrain. Blizzard (10, `Targets 0x40`, family 3 mask `0x80080`, row 4, 0 to
    /// 30 yd) under Arctic Reach's cells, and a melee-row ground spell (12684, row 2) under the
    /// caster's reach and its auto-attack target's.
    #[test]
    fn the_terrain_verdict_reads_the_range_talent_and_the_casters_reaches() {
        use super::super::corpse_fixture as fx;
        use crate::creature_anim::Engaged;
        use bevy::ecs::system::RunSystemOnce;
        use std::collections::HashMap;

        const BLIZZARD: u32 = 10;
        const MELEE_GROUND: u32 = 12684;
        const ATTACK_TARGET: u64 = 0xF130_0000_0000_0007;
        // A spell, a talent or not, the caster's reach, an engaged target's reach, the distance.
        let verdict =
            |spell: u32, talent: bool, self_reach: f32, engaged: Option<f32>, d: f32| -> bool {
                let mut world = World::new();
                world.init_resource::<WorldCursor>();
                world.init_resource::<SpellTargeting>();
                world.init_resource::<crate::target::Hovered>();
                world.init_resource::<crate::target::HoveredObject>();
                world.init_resource::<crate::go_templates::GameObjectTemplates>();
                world.init_resource::<crate::items::Items>();
                world.insert_resource(crate::net::Reputations(Vec::new()));
                world.insert_resource(PickOcclusion {
                    distance: 10.0,
                    point: Some(Vec3::new(d, 0.0, 0.0)),
                });
                let mut mods = SpellModifiers::default();
                mods.set_class_family(3);
                if talent {
                    for bit in [5, 7] {
                        mods.set(false, bit, crate::spell::OP_RANGE, 20);
                    }
                }
                world.insert_resource(mods);
                let display = |range_index, flags| SpellDisplay {
                    range_index,
                    targets: 0x40,
                    spell_family: 3,
                    spell_family_flags: flags,
                    ..SpellDisplay::default()
                };
                let mut spells = Spells::empty_for_tests();
                spells.catalog = benilla_formats::SpellCatalog::from_displays(HashMap::from([
                    (BLIZZARD, display(4, 0x8_0080)),
                    (MELEE_GROUND, display(2, 0)),
                ]));
                spells.ranges = benilla_formats::SpellRangeCatalog::from_rows(HashMap::from([
                    (
                        4,
                        SpellRange {
                            min: 0.0,
                            max: 30.0,
                            flags: 0,
                        },
                    ),
                    (
                        2,
                        SpellRange {
                            min: 0.0,
                            max: 5.0,
                            flags: 1,
                        },
                    ),
                ]));
                world.insert_resource(spells);
                let mut index = HashMap::new();
                let mut me =
                    world.spawn((SelfPlayer, Transform::default(), fx::caster(self_reach)));
                if engaged.is_some() {
                    me.insert(Engaged(ATTACK_TARGET));
                }
                if let Some(reach) = engaged {
                    let target = world.spawn(fx::caster(reach)).id();
                    index.insert(ATTACK_TARGET, target);
                }
                world.insert_resource(crate::net::GuidIndex(index));
                world.resource_mut::<SpellTargeting>().enter(
                    spell,
                    crate::spell::CastCommit::Spell,
                    0x0040,
                );
                world
                    .run_system_once(drive_targeting_cursor)
                    .expect("the targeting cursor drives");
                !world.resource::<WorldCursor>().unable
            };

        // 30 yd without the talent, 36 with it.
        assert!(!verdict(BLIZZARD, false, 1.5, None, 33.0), "past 30 yd");
        assert!(verdict(BLIZZARD, true, 1.5, None, 33.0), "inside 36 yd");
        assert!(!verdict(BLIZZARD, true, 1.5, None, 37.0), "past 36 yd");
        // The melee row's 5.0 floor; the caster's own reach twice, 3.0 + 3.0 + 1.3333; an auto-attack
        // target's 4.0 in place of the second, 1.5 + 4.0 + 1.3333.
        assert!(!verdict(MELEE_GROUND, false, 1.5, None, 7.0));
        assert!(verdict(MELEE_GROUND, false, 3.0, None, 7.0));
        assert!(verdict(MELEE_GROUND, false, 1.5, Some(4.0), 6.5));
        assert!(!verdict(MELEE_GROUND, false, 1.5, Some(4.0), 7.0));
    }

    /// `0x6e6460`'s GameObject leg asks the same range through the same call: a chest a lock spell
    /// opens, `d` yd out, on a family-3 lock spell (fixture) with row 12, 0 to 5 yd.
    #[test]
    fn the_object_leg_reads_the_range_talent() {
        use super::super::corpse_fixture as fx;
        use benilla_formats::{LockCatalog, LockSlot, OpenLock, LOCK_KEY_SKILL};
        use bevy::ecs::system::RunSystemOnce;
        use std::collections::HashMap;

        const OPENER: u32 = 6477;
        const LOCK: u32 = 55;
        // Entry 7, a chest whose `data[0]` is the lock.
        const CHEST: u64 = 0xF110_0000_0000_0001 | 7 << 24;
        let verdict = |talent: bool, d: f32| -> bool {
            let mut world = World::new();
            world.init_resource::<WorldCursor>();
            world.init_resource::<SpellTargeting>();
            world.init_resource::<crate::target::Hovered>();
            world.init_resource::<crate::items::Items>();
            world.init_resource::<crate::net::GuidIndex>();
            world.insert_resource(crate::net::Reputations(Vec::new()));
            world.init_resource::<PickOcclusion>();
            let mut mods = SpellModifiers::default();
            mods.set_class_family(3);
            if talent {
                mods.set(false, 7, crate::spell::OP_RANGE, 20);
            }
            world.insert_resource(mods);
            let mut spells = Spells::empty_for_tests();
            spells.catalog = benilla_formats::SpellCatalog::from_displays(HashMap::from([(
                OPENER,
                SpellDisplay {
                    range_index: 12,
                    targets: 0x4000,
                    spell_family: 3,
                    spell_family_flags: 1 << 7,
                    open_lock: Some(OpenLock {
                        effect: 0,
                        lock_type: 1,
                    }),
                    ..SpellDisplay::default()
                },
            )]));
            spells.ranges = benilla_formats::SpellRangeCatalog::from_rows(HashMap::from([(
                12,
                SpellRange {
                    min: 0.0,
                    max: 5.0,
                    flags: 0,
                },
            )]));
            world.insert_resource(spells);
            let mut slots = [LockSlot::default(); benilla_formats::MAX_LOCK_SLOTS];
            slots[0] = LockSlot {
                key_type: LOCK_KEY_SKILL,
                index: 1,
                skill: 0,
                action: 5,
            };
            world.insert_resource(crate::go_templates::Locks(LockCatalog::from_rows([(
                LOCK, slots,
            )])));
            let mut templates = crate::go_templates::GameObjectTemplates::default();
            let mut data = [0; 24];
            data[0] = LOCK as i32;
            templates.insert(7, 3, "Chest".into(), &data);
            world.insert_resource(templates);
            world.spawn((SelfPlayer, Transform::default(), fx::caster(1.5)));
            let chest = world
                .spawn(Transform::from_translation(Vec3::new(d, 0.0, 0.0)))
                .id();
            world.insert_resource(crate::target::HoveredObject {
                target: Some(chest),
                guid: Some(CHEST),
                distance: 5.0,
            });
            world.resource_mut::<SpellTargeting>().enter(
                OPENER,
                crate::spell::CastCommit::Spell,
                0x4800,
            );
            world
                .run_system_once(drive_targeting_cursor)
                .expect("the targeting cursor drives");
            !world.resource::<WorldCursor>().unable
        };

        // 5 yd without the talent, 6 with it.
        assert!(verdict(false, 4.5));
        assert!(!verdict(false, 5.5), "past 5 yd");
        assert!(verdict(true, 5.5), "inside 6 yd");
        assert!(!verdict(true, 6.5), "past 6 yd");
    }

    /// `0x6e6460`'s unit leg over a hovered unit: the relation checks, then min² ≤ d² ≤ max²
    /// (`6e677c`–`6e6802`). Row 5 is 0 to 30 yd and row 114 is 8 to 35 yd, both padded by the two
    /// 1.5 combat reaches: 0 to 33 and 11 to 38.
    #[test]
    fn the_unit_leg_is_grey_out_of_range_or_off_relation() {
        use bevy::ecs::system::RunSystemOnce;
        use std::collections::HashMap;

        const HEAL: u32 = 2050;
        const SHOT: u32 = 75;
        let verdict_for = |spell: u32, word: u16, distance: f32| {
            let mut world = World::new();
            world.init_resource::<WorldCursor>();
            world.init_resource::<SpellTargeting>();
            world.init_resource::<crate::target::HoveredObject>();
            world.init_resource::<crate::go_templates::GameObjectTemplates>();
            world.init_resource::<crate::items::Items>();
            world.init_resource::<crate::net::GuidIndex>();
            world.init_resource::<crate::spell::SpellModifiers>();
            world.insert_resource(crate::net::Reputations(Vec::new()));
            world.init_resource::<PickOcclusion>();
            let display = |range_index| benilla_formats::SpellDisplay {
                range_index,
                ..Default::default()
            };
            let row = |min, max| SpellRange { min, max, flags: 0 };
            let mut spells = Spells::empty_for_tests();
            spells.catalog = benilla_formats::SpellCatalog::from_displays(HashMap::from([
                (HEAL, display(5)),
                (SHOT, display(114)),
            ]));
            spells.ranges = benilla_formats::SpellRangeCatalog::from_rows(HashMap::from([
                (5, row(0.0, 30.0)),
                (114, row(8.0, 35.0)),
            ]));
            world.insert_resource(spells);
            // `UNIT_FIELD_HEALTH` 100: a store with no health reads dead to `BindTarget`'s gates.
            let live = || {
                crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(22, 100)]))
            };
            world.spawn((
                SelfPlayer,
                Transform::default(),
                GlobalTransform::default(),
                live(),
            ));
            let unit = world
                .spawn((
                    GlobalTransform::from_translation(Vec3::new(distance, 0.0, 0.0)),
                    live(),
                ))
                .id();
            world.insert_resource(crate::target::Hovered {
                target: Some(unit),
                guid: Some(0xF130_0000_0000_0001),
                distance: 5.0,
                ..Default::default()
            });
            world.resource_mut::<SpellTargeting>().enter(
                spell,
                crate::spell::CastCommit::Spell,
                word,
            );
            world
                .run_system_once(drive_targeting_cursor)
                .expect("the targeting cursor drives");
            !world.resource::<WorldCursor>().unable
        };
        let verdict = |word, distance| verdict_for(HEAL, word, distance);

        // `TARGET_FLAG_UNIT` (0x2) binds any unit, so only range decides.
        assert!(verdict(0x0002, 10.0), "a valid unit in range → Cast");
        assert!(
            !verdict(0x0002, 40.0),
            "the same unit out of range → UnableCast"
        );
        // An assist word over a unit with no faction catalog: neutral, not assistable.
        assert!(
            !verdict(0x0100, 10.0),
            "a unit that fails the relation → UnableCast"
        );
        // Inside the minimum is out of range too.
        assert!(verdict_for(SHOT, 0x0002, 20.0), "inside the band → Cast");
        assert!(
            !verdict_for(SHOT, 0x0002, 5.0),
            "inside the minimum → UnableCast"
        );
    }

    /// The unit leg's range compare, the tail of `SpellCanTargetUnit 0x6e6460`, hands
    /// `GetMinMaxRange` the hovered unit (`0x6e679b`, `0x6e67d1`): a melee spell is in range to
    /// 7.667 yards while the caster and that unit both run, and to the 5.0 floor otherwise.
    #[test]
    fn the_unit_leg_takes_the_moving_bonus_on_the_melee_row() {
        use bevy::ecs::system::RunSystemOnce;
        use std::collections::HashMap;

        const HAMSTRING: u32 = 1715;
        let verdict = |caster_runs: bool, unit_runs: bool, distance: f32| {
            let mut world = World::new();
            world.init_resource::<WorldCursor>();
            world.init_resource::<SpellTargeting>();
            world.init_resource::<crate::target::HoveredObject>();
            world.init_resource::<crate::go_templates::GameObjectTemplates>();
            world.init_resource::<crate::items::Items>();
            world.init_resource::<crate::net::GuidIndex>();
            world.init_resource::<crate::spell::SpellModifiers>();
            world.insert_resource(crate::net::Reputations(Vec::new()));
            world.init_resource::<PickOcclusion>();
            world.insert_resource(crate::player::Player::with_move_flags(if caster_runs {
                crate::creature_anim::move_flags::FORWARD
            } else {
                0
            }));
            let mut spells = Spells::empty_for_tests();
            spells.catalog = benilla_formats::SpellCatalog::from_displays(HashMap::from([(
                HAMSTRING,
                benilla_formats::SpellDisplay {
                    range_index: 2,
                    ..Default::default()
                },
            )]));
            spells.ranges = benilla_formats::SpellRangeCatalog::from_rows(HashMap::from([(
                2,
                SpellRange {
                    min: 0.0,
                    max: 5.0,
                    flags: 1,
                },
            )]));
            world.insert_resource(spells);
            let speeds = crate::net::UnitSpeeds(benilla_protocol::MoveSpeeds {
                walk: 2.5,
                run: 7.0,
                run_back: 4.5,
                swim: 4.7,
                swim_back: 2.5,
                turn_rate: 3.1,
            });
            let live = || {
                crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(22, 100)]))
            };
            world.spawn((
                SelfPlayer,
                crate::net::Embodied,
                speeds,
                Transform::default(),
                GlobalTransform::default(),
                live(),
            ));
            let unit = world
                .spawn((
                    speeds,
                    GlobalTransform::from_translation(Vec3::new(distance, 0.0, 0.0)),
                    live(),
                ))
                .id();
            if unit_runs {
                world.entity_mut(unit).insert(crate::net::Spline {
                    points: vec![[distance, 0.0, 0.0], [distance + 16.0, 0.0, 0.0]],
                    start: std::time::Instant::now(),
                    duration: std::time::Duration::from_secs(2),
                    id: 1,
                    grounded: true,
                    run_mode: true,
                    deck: None,
                });
            }
            world.insert_resource(crate::target::Hovered {
                target: Some(unit),
                guid: Some(0xF130_0000_0000_0001),
                distance: 5.0,
                ..Default::default()
            });
            world.resource_mut::<SpellTargeting>().enter(
                HAMSTRING,
                crate::spell::CastCommit::Spell,
                0x0002,
            );
            world
                .run_system_once(drive_targeting_cursor)
                .expect("the targeting cursor drives");
            !world.resource::<WorldCursor>().unable
        };
        assert!(verdict(true, true, 7.5), "both running: inside 7.667");
        assert!(!verdict(true, true, 8.0), "past it");
        assert!(!verdict(false, true, 7.5), "the caster stands");
        assert!(!verdict(true, false, 7.5), "the unit stands");
        assert!(verdict(false, false, 4.9), "the floor");
    }

    /// `0x6e6460`'s corpse leg over a hovered corpse: bones, a hostile corpse under the ally bit
    /// and a corpse out of range are grey. The range pads a corpse by the caster's reach twice
    /// (`6e3605`–`6e361e`): Resurrection's 30 yd with a 3.0 reach is 36.
    #[test]
    fn the_corpse_leg_is_grey_for_bones_a_hostile_corpse_or_out_of_range() {
        use super::super::corpse_fixture as fx;
        use bevy::ecs::system::RunSystemOnce;

        let verdict = |word: u16, corpse: crate::net::ObjectStore, distance: f32| {
            let mut world = World::new();
            world.init_resource::<WorldCursor>();
            world.init_resource::<SpellTargeting>();
            world.init_resource::<crate::target::HoveredObject>();
            world.init_resource::<crate::go_templates::GameObjectTemplates>();
            world.init_resource::<crate::items::Items>();
            world.init_resource::<crate::net::GuidIndex>();
            world.init_resource::<crate::spell::SpellModifiers>();
            world.insert_resource(crate::net::Reputations(Vec::new()));
            world.init_resource::<PickOcclusion>();
            world.insert_resource(fx::spells());
            world.insert_resource(fx::factions());
            world.spawn((
                SelfPlayer,
                Transform::default(),
                GlobalTransform::default(),
                fx::caster(3.0),
            ));
            let body = world
                .spawn((
                    GlobalTransform::from_translation(Vec3::new(distance, 0.0, 0.0)),
                    corpse,
                ))
                .id();
            world.insert_resource(crate::target::Hovered {
                corpse: Some(body),
                corpse_guid: Some(fx::CORPSE),
                distance: 5.0,
                ..Default::default()
            });
            world.resource_mut::<SpellTargeting>().enter(
                fx::RESURRECTION,
                crate::spell::CastCommit::Spell,
                word,
            );
            world
                .run_system_once(drive_targeting_cursor)
                .expect("the targeting cursor drives");
            !world.resource::<WorldCursor>().unable
        };
        let friend = || fx::corpse(fx::HUMAN, false);

        assert!(verdict(0x8000, friend(), 10.0), "a friend's corpse → Cast");
        assert!(
            !verdict(0x8000, fx::corpse(fx::HUMAN, true), 10.0),
            "bones → UnableCast"
        );
        assert!(
            !verdict(0x8000, fx::corpse(fx::ORC, false), 10.0),
            "a hostile corpse under the ally bit → UnableCast"
        );
        assert!(
            verdict(0x0200, fx::corpse(fx::ORC, false), 10.0),
            "which the enemy bit takes"
        );
        // Past the bare 30 and past 30 + 3.0 + 1.5, inside 30 + 2 × 3.0.
        assert!(verdict(0x8000, friend(), 35.0), "inside the padded range");
        assert!(
            !verdict(0x8000, friend(), 37.0),
            "out of range → UnableCast"
        );
        // A word outside `0x8600` has no corpse leg: the heal word over a corpse is state 0.
        assert!(!verdict(0x0002, friend(), 10.0));
    }
}
