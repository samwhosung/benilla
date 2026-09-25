//! What the player *sees* while a cast waits for its click — the two classifier pre-empts and the
//! numbers they compute: the ground point's range verdict (`CheckGroundPointInRange 0x6e6810`,
//! inside `0x4820f0`), the hovered object's validity
//! (`0x6e6460`, inside `0x4828d0`), and the reticle's radius
//! (`GetCurrentCastRadius 0x6e6350`).
//!
//! One module rather than a branch inside each seam, because in the reference this is one
//! decision made in one place: the **pick** picks the handler, the pick flags come from the word,
//! and every path that is not a handler ends at UnableCast. See [`drive_targeting_cursor`] for the
//! table.

use bevy::prelude::*;

use benilla_formats::SpellRange;

use crate::net::SelfPlayer;
use crate::target::{CursorKind, PickOcclusion, WorldCursor};
use crate::ui_action::Spells;

use super::{SpellTargeting, TargetingWants};

/// `CheckGroundPointInRange 0x6e6810` — min²/max² from the spell's `SpellRange` row against the
/// squared caster↔point distance. Its ONE caller binary-wide is the hover-cursor classifier
/// (`0x4820f0`): the verdict colours
/// Cast/UnableCast and nothing else. The click never consults it, so neither does ours. No row
/// (a failed DBC, an unknown spell) is permissive — the server validates every send anyway.
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

/// The targeting spell's `SpellRange` row, through the catalogs.
fn range_row(spells: Option<&Spells>, spell_id: u32) -> Option<&SpellRange> {
    let spells = spells?;
    spells.ranges.get(spells.catalog.get(spell_id)?.range_index)
}

/// `GetCurrentCastRadius 0x6e6350` — the reticle's
/// radius: per-effect `radius + casterLevel × perLevel` over **EffectRadiusIndex[0] and [1]
/// only** (slot 2 is never read by the client), the max with candidate 1 winning ties/NaN,
/// clamped to 20.0 (`0x4820f0`'s `[0x804478]` literal — `min`, NaN → 20). `0.0` = no radius
/// rows; the reticle then draws at its literal default size. Spell-mod op 6 (SPELLMOD_RADIUS) is
/// not read here — the tables are live (`crate::spell::mods`), this consumer is not wired to
/// them (the 0792 residual, same as the range gate).
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
    // Strict > for candidate 0; a tie or a NaN c0 falls to candidate 1 — the byte order.
    let r = if c0 > c1 { c0 } else { c1 };
    r.min(20.0)
}

/// While targeting, the world cursor is the classifier's pre-empt (`0x4820f0`). Runs right
/// after [`crate::target`]'s classifier in the target chain and overwrites its verdict.
///
/// **That pre-emption is the WORLD's, and only the world's**. This computes
/// the world's verdict every frame — [`crate::target::reticle`] reads `WorldCursor.unable` as the
/// AoE ring's colour, so it must stay live even while the mouse is parked on a bag. What the
/// reference gates is the *display*: its hover handler `0x481790` runs only while the WorldFrame is
/// the frame manager's mouse-focus frame, so over a UI frame the cursor simply keeps its last
/// value. That gate lives in [`crate::cursor`], over the one sticky mode.
///
/// **The verdict is per-seam, and the default is grey**. The reference reaches a
/// cursor through the pick, and while targeting the pick flags come from the word alone
/// (`0x481050`'s targeting arm), so the *word* chooses which of three handlers runs — and the
/// third one is the reason this function is not just a range check:
///
/// | pick state | handler | verdict |
/// |---|---|---|
/// | 1 — terrain | `0x4820f0` @ `48214b` | `CheckGroundPointInRange 0x6e6810` over the ground point |
/// | 2 — object | `0x4828d0` @ `482910` → `0x6e6460` | the hovered object's own validity |
/// | 0 — nothing hit | `0x481790`'s tail | **`CursorSetMode(0x16)` = UnableCast** |
///
/// That last row is the one that makes an armed lockpick read right: a word without `& 0x60` sets
/// no PF bit `0x1`, so a terrain-only hit is suppressed to state 0 and the cursor is **grey
/// everywhere except over a GameObject it can actually open**. An item-only word (a poison, an
/// enchant — `0x0010`) yields PF `0` outright, the pick bails before building a ray
/// (`0x4812c8`), and the world cursor is grey for the whole time it is armed — correct, because
/// that word's click lives in the bag, not the world.
///
/// The object arm is `0x6e6460`'s GameObject leg (`6e66f3`: the picked object's typemask bit 5):
/// `word & 0x4800` (`6e6702 testb $0x48, %ah`), then
/// [`crate::target::lock::spell_opens_lock`] (`6e670f call 0x5f8260`), then — because the call
/// site passes `dl = 1` — the same min²/max² range test the ground arm uses (`6e677c..6e6801`,
/// through the very `GetMinMaxRange 0x6e3480` that `0x6e6810` calls, so the two arms carry the
/// identical [`range_row`] simplification and no new residual).
///
/// `0x6e6460`'s other legs are unreachable from our targeting mode and deliberately not
/// transcribed: the **unit** leg (`6e6519`) — a unit-target spell never enters targeting mode at
/// all, it resolves to `CastWireTarget::Unit` — the world-CGItem leg (`6e66de`), and the corpse
/// leg (`6e6719`). Named in.
///
/// The cursor is still a **whole-word** surface in one respect — every seam shows the `Cast`
/// *kind*, only `unable` differs — which is why it reads [`SpellTargeting::spell`]. The reticle is
/// per-seam and reads [`SpellTargeting::spell_for`].
pub(crate) fn drive_targeting_cursor(
    targeting: Res<SpellTargeting>,
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
    // Read-only here: the ask-once template request is made at object stream-in
    // (`net::objects`), never by this hover path, so a cold cache is a one-frame transient
    // and not a permanent grey.
    lock_inputs: crate::target::lock::GoLockInputs,
    mut cursor: ResMut<WorldCursor>,
) {
    let Some(spell_id) = targeting.spell() else {
        return;
    };
    let row = range_row(spells.as_deref(), spell_id);
    let me = self_tf.single().ok().map(|tf| tf.translation);
    // The pick's own arbitration decides which handler runs, and a GameObject hit is already
    // occlusion-filtered — so "a GO is the nearest pick" is exactly the condition the click uses
    // ([`super::world::commit_object_cast_on_click`]). Cursor and click ask one question.
    let able = if targeting.wants(TargetingWants::GameObject)
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
        // `0x4820f0` — the ground point's range verdict. No hit (sky, mouselook) is state 0 for
        // this word too, and state 0 while targeting is already UnableCast.
        match (occlusion.point, me) {
            (Some(point), Some(me)) => ground_point_in_range(row, me, point),
            _ => false,
        }
    } else {
        // Pick state 0 — nothing this word can bind is under the cursor.
        false
    };
    *cursor = WorldCursor {
        kind: CursorKind::Cast,
        unable: !able,
    };
}

/// `0x6e6460`'s GameObject leg: the word's `& 0x4800`, the spell-vs-lock predicate `0x5f8260`, and
/// the `dl = 1` range test. Split out only so the dispatch above reads like the reference's table.
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
    // `0x5f8260`'s two data lookups: the GO's template → its `Lock.dbc` row. A template still in
    // flight is not yet openable — grey, not lit; the stream-in query makes that a rare frame.
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
    // The `dl = 1` tail (`6e677c`): the caster↔target distance against the spell's own min/max.
    match (me, go_tf.get(entity)) {
        (Some(me), Ok(tf)) => ground_point_in_range(row, me, tf.translation),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `0x6e6810` mirror: min²/max² against the squared caster↔point distance — the
    /// CURSOR's verdict and nothing else (its one caller binary-wide is the hover classifier;
    /// the click never asks). Permissive with no row (Blizzard's row 4 is 0–30 yd; a synthetic
    /// min exercises the too-close arm the real row can't).
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

    /// `GetCurrentCastRadius 0x6e6350` + the `0x4820f0` clamp: slots 0/1 only (slot 2 is never
    /// read), max with candidate-1 winning ties, per-level scaling, min(r, 20). Fixture rows
    /// mirror the real table (row 14 = 8.0 Blizzard, row 8 = 5.0 Flamestrike).
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
            (777, display([0, 0, 13])), // slot 2 only — the client never reads it
            (778, display([90, 8, 0])), // per-level row in slot 0
            (779, display([10, 0, 0])), // row 10 = 30.0 — the 20.0 clamp
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
        // Slot 2 is invisible to the reticle — no rows in 0/1 reads 0 (→ the default size).
        assert_eq!(ground_cast_radius(s, 777, 60), 0.0);
        // Per-level: 2.0 + 60 × 0.1 = 8.0 beats slot 1's 5.0.
        assert_eq!(ground_cast_radius(s, 778, 60), 8.0);
        // The 20.0 clamp (`[0x804478]`).
        assert_eq!(ground_cast_radius(s, 779, 60), 20.0);
        // Unknown spell / no data at all → 0 (default size).
        assert_eq!(ground_cast_radius(s, 9999, 60), 0.0);
        assert_eq!(ground_cast_radius(None, 10, 60), 0.0);
    }

    /// **The dispatch table** — the reference's three pick states, and the fact
    /// that only two of them are handlers. Before this, every seam took plain `Cast`, so an armed
    /// poison or lockpick showed a lit cast cursor over open ground it could do nothing with.
    ///
    /// The load-bearing row is the last one: *state 0 is UnableCast*. A word that wants no
    /// location sets no PF bit `0x1`, so bare ground can never be a handler for it.
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

        // Pick state 1 — Blizzard's DEST word over a ground point. No `Spells` resource means no
        // range row, and a rowless spell is permissive (the server still judges).
        assert!(
            verdict(0x0040, Some(Vec3::ZERO), None),
            "ground point → Cast"
        );
        // …and over sky / mouselook there is no point: state 0.
        assert!(!verdict(0x0040, None, None), "no ground hit → UnableCast");

        // An **item-only** word (a poison, an enchant, `0x0010`): PF is 0, the pick bails at
        // `0x4812c8` before it builds a ray, so the world cursor is grey the whole time it is
        // armed — with or without ground under the mouse.
        assert!(!verdict(0x0010, Some(Vec3::ZERO), None));
        assert!(!verdict(0x0010, None, None));
        // Even with a GameObject under the cursor: `& 0x4800` is 0, so the object arm is not its
        // handler either.
        assert!(!verdict(0x0010, Some(Vec3::ZERO), Some(3.0)));

        // A **GameObject** word (Opening, `0x4800`) over bare ground — no bit `0x1`, state 0.
        assert!(
            !verdict(0x4800, Some(Vec3::ZERO), None),
            "lockpick over dirt is grey"
        );
        // Over a GameObject whose template has not streamed in yet: the object arm bails, and a
        // bail is grey rather than lit — the click is what the server judges, not the cursor.
        assert!(!verdict(0x4800, Some(Vec3::ZERO), Some(3.0)));

        // A lock word that ALSO carries DEST (`0x4840`) still has its terrain handler when no
        // GameObject is the nearest pick — the seams are questions, not a partition.
        assert!(verdict(0x4840, Some(Vec3::ZERO), None));
    }
}
