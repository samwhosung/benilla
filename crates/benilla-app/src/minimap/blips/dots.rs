//! The `ObjectIcons.blp` dots of the classifier `0x4eaa90`: quest gold cell 3, tracking gold and
//! red cells 0 and 1, party blue cell 4.

use std::collections::HashMap;

use bevy::math::Rect;
use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::{LockCatalog, ShapeshiftForm, LOCK_KEY_SKILL};
use benilla_protocol::EntityKind;

use crate::go_templates::GameObjectTemplates;
use crate::names::NameCache;
use crate::net::{GuidIndex, NetEntity, ObjectStore};
use crate::ui_pass::{UiQuad, UiQuads, UvRect};

use super::{party_member_pos, BlipCtx, MinimapBlipHover, TrackedCandidates, BLIP_BASIS_PX};

/// The quest dot's quad, 8 px (`0xbc82a8`, frozen by the ctor); the per-cell scale table
/// `{1,1,1,1,1.3}` scales only the party cell 4.
const QUEST_DOT_PX: f32 = 8.0;
/// The party dot's quad: the same 8-px base at the cell table's 1.3× party scale.
const PARTY_DOT_PX: f32 = 8.0 * 1.3;
/// `ObjectIcons.blp` cell 4, the blue party dot (col 0, row 1 of the 4×4 grid).
const PARTY_DOT_CELL: [f32; 4] = [0.0, 0.25, 0.25, 0.5];
/// `ObjectIcons.blp` cell 0, the gold dot of a GameObject passing resource tracking (`0x5ed2b0`).
const TRACKED_GO_CELL: [f32; 4] = [0.0, 0.25, 0.0, 0.25];
/// `ObjectIcons.blp` cell 1, the red dot of a unit passing creature tracking (`0x5ed210`).
const TRACKED_UNIT_CELL: [f32; 4] = [0.25, 0.5, 0.0, 0.25];
/// `UNIT_DYNAMIC_FLAGS` bit 0x2, set per viewer on a Hunter's Mark victim for its caster; one of
/// `0x5ed210`'s two always-show clauses (`+0x224 & 0x2`).
const UNIT_DYNFLAG_TRACK_UNIT: u32 = 0x2;
/// Creature type 7, Humanoid: a player's type (`ChrRaces.dbc` col 9 is 7 for all nine playable
/// races, read by `0x605570`) and the `<= 0` fallback of the shapeshift override.
const CREATURE_TYPE_HUMANOID: u32 = 7;

/// Only DIALOG_STATUS 7 draws a quest dot, the gold cell 3 (`cmp [obj+0xcb8],7` at `0x4eac31`).
fn quest_dot_cell(status: u32) -> Option<[f32; 4]> {
    (status == 7).then_some([0.75, 1.0, 0.0, 0.25])
}

/// The three gates `0x4eaa90` applies to a unit or player ahead of the status-7 compare at
/// `0x4eac31`, so they bar the quest dot and the tracking dot alike, never a GameObject or party
/// dot. A unit with no descriptor yet fails, as a zeroed health field does in the reference.
fn unit_dot_eligible(store: Option<&ObjectStore>, me: Option<u64>) -> bool {
    let Some(f) = store.map(|s| &s.0) else {
        return false;
    };
    // 1. A signed `<= 0` on `UNIT_FIELD_HEALTH` (`jle` at `0x4eac1e`): the dead get no dot.
    if f.unit_health().unwrap_or(0) as i32 <= 0 {
        return false;
    }
    // 2. The owner, `UNIT_FIELD_CHARMEDBY` when non-zero else `UNIT_FIELD_SUMMONEDBY`, equal to
    //    our guid (`0x468550`) rejects: our own pet, minion or charmed unit is never a blip.
    let owner = match f.unit_charmed_by() {
        Some(g) if g != 0 => g,
        _ => f.unit_summoned_by().unwrap_or(0),
    };
    if owner != 0 && Some(owner) == me {
        return false;
    }
    // 3. `byte [eax+0x213] & 4`, `UNIT_FIELD_BYTES_1` byte 3 bit 2 (vmangos's untrackable name).
    !f.unit_is_untrackable()
}

/// Draw a gold dot per quest giver at status 7, culled at the view radius in 3-D distance with
/// no rim arrow. Runs after the player arrow, so the dots draw on top.
pub(in crate::minimap) fn emit_quest_dots(
    ctx: &BlipCtx,
    statuses: &HashMap<u64, u32>,
    candidates: &TrackedCandidates,
    self_guid: Option<u64>,
    icons: &Handle<Image>,
    player_indoors: bool,
    unit_indoors: impl Fn(Vec3) -> bool,
    quads: &mut UiQuads,
    hover: &mut MinimapBlipHover,
) {
    for (guid, net, tf, store) in candidates.iter() {
        let npc = guid.0;
        if !matches!(net.kind, EntityKind::Unit | EntityKind::Player) {
            continue; // the GameObject leg (`0x4eab43`) never reaches the status-7 compare
        }
        let Some(cell) = statuses.get(&npc).copied().and_then(quest_dot_cell) else {
            continue;
        };
        if !unit_dot_eligible(store, self_guid) {
            continue;
        }
        let w = bevy_to_wow(tf.translation());
        let d3 =
            ((w[0] - ctx.wx).powi(2) + (w[1] - ctx.wy).powi(2) + (w[2] - ctx.wz).powi(2)).sqrt();
        if d3 > ctx.radius_yd {
            continue;
        }
        // The grey `0xffb0b0b0` marks a dot across an interior boundary; the reference decides
        // through `0x670540`, untraced, and this uses an indoor-containment mismatch.
        let grey = unit_indoors(tf.translation()) != player_indoors;
        let tint = if grey { 0xb0 as f32 / 255.0 } else { 1.0 };
        let rect = Rect::from_center_size(
            ctx.center + ctx.offset(w),
            Vec2::splat(ctx.side * (QUEST_DOT_PX / BLIP_BASIS_PX)),
        );
        quads.overlays.push(UiQuad {
            rect,
            z_key: ctx.z,
            texture: Some(icons.clone()),
            uv: UvRect::from_tex_coords(cell),
            color: [tint, tint, tint, ctx.alpha],
            ..default()
        });
        if let (Some(c), Some(ui)) = (ctx.cursor, ctx.cursor_ui) {
            if rect.contains(c) {
                *hover = MinimapBlipHover::Npc(npc, ui, grey);
            }
        }
    }
}

/// Resource tracking (`0x5ed2b0`): any skill-keyed `Lock.dbc` slot of the lock whose `LockType`
/// `n` has bit `1 << (n − 1)` set in `PLAYER_TRACK_RESOURCES`, the bit the server sets from the
/// aura's MiscValue (vmangos `HandleAuraTrackResources`).
fn tracked_resource(mask: u32, lock_id: u32, locks: &LockCatalog) -> bool {
    if mask == 0 || lock_id == 0 {
        return false;
    }
    locks.slots(lock_id).is_some_and(|slots| {
        slots.iter().any(|s| {
            s.key_type == LOCK_KEY_SKILL
                && (1..=32).contains(&s.index)
                && mask & (1u32 << (s.index - 1)) != 0
        })
    })
}

/// Our tracking masks and the track-stealthed bit (`PLAYER_FIELD_BYTES & 0x2`).
#[derive(Clone, Copy, Default)]
pub(in crate::minimap) struct SelfTracking {
    pub(in crate::minimap) creatures: u32,
    pub(in crate::minimap) resources: u32,
    pub(in crate::minimap) stealthed: bool,
}

/// Creature tracking (`0x5ed210`): either always-show clause, `UNIT_DYNFLAG_TRACK_UNIT` or our
/// track-stealthed bit (aura 151) with the unit's CREEP flag, else the creature type's bit
/// `type − 1` in `PLAYER_TRACK_CREATURES`. The predicate itself tests no alive/dead or faction.
fn tracked_creature(
    tracking: SelfTracking,
    creature_type: Option<u32>,
    dynamic_flags: u32,
    unit_creeping: bool,
) -> bool {
    if dynamic_flags & UNIT_DYNFLAG_TRACK_UNIT != 0 {
        return true;
    }
    if tracking.stealthed && unit_creeping {
        return true;
    }
    tracking.creatures != 0
        && creature_type
            .is_some_and(|t| (1..=32).contains(&t) && tracking.creatures & (1u32 << (t - 1)) != 0)
}

/// The creature-type resolver `0x605570`: a shapeshift form's `SpellShapeshiftForm.dbc` type
/// first (`<= 0` reads Humanoid), else an NPC's cached template, else Humanoid for a player.
fn creature_type_of(
    kind: EntityKind,
    shapeshift_form: u8,
    entry: Option<u32>,
    names: &NameCache,
    forms: Option<&HashMap<u32, ShapeshiftForm>>,
) -> Option<u32> {
    if shapeshift_form != 0 {
        if let Some(t) =
            forms.and_then(|f| f.get(&u32::from(shapeshift_form)).map(|r| r.creature_type))
        {
            return Some(if t >= 1 {
                t as u32
            } else {
                CREATURE_TYPE_HUMANOID
            });
        }
    }
    match kind {
        EntityKind::Unit => entry.and_then(|e| names.creature_type(e)),
        EntityKind::Player => Some(CREATURE_TYPE_HUMANOID),
        _ => None,
    }
}

/// Draw the tracking dots, gold cell 0 per GameObject then red cell 1 per unit not at quest
/// status 7 (`0x4eac31` tests that first). Drawn before the quest and party dots, as the cell
/// lists draw in order.
pub(in crate::minimap) fn emit_tracking_dots(
    ctx: &BlipCtx,
    tracking: SelfTracking,
    candidates: &TrackedCandidates,
    statuses: &HashMap<u64, u32>,
    self_guid: Option<u64>,
    names: &NameCache,
    templates: &GameObjectTemplates,
    locks: Option<&LockCatalog>,
    forms: Option<&HashMap<u32, ShapeshiftForm>>,
    icons: &Handle<Image>,
    player_indoors: bool,
    unit_indoors: impl Fn(Vec3) -> bool,
    quads: &mut UiQuads,
    hover: &mut MinimapBlipHover,
) {
    // The unit pass runs even with an empty creature mask: the always-show clauses need none.
    let mut dot = |guid: u64, tf: &GlobalTransform, cell: [f32; 4], name: DotName| {
        let w = bevy_to_wow(tf.translation());
        let d3 =
            ((w[0] - ctx.wx).powi(2) + (w[1] - ctx.wy).powi(2) + (w[2] - ctx.wz).powi(2)).sqrt();
        if d3 > ctx.radius_yd {
            return;
        }
        let grey = unit_indoors(tf.translation()) != player_indoors;
        let tint = if grey { 0xb0 as f32 / 255.0 } else { 1.0 };
        let rect = Rect::from_center_size(
            ctx.center + ctx.offset(w),
            Vec2::splat(ctx.side * (QUEST_DOT_PX / BLIP_BASIS_PX)),
        );
        quads.overlays.push(UiQuad {
            rect,
            z_key: ctx.z,
            texture: Some(icons.clone()),
            uv: UvRect::from_tex_coords(cell),
            color: [tint, tint, tint, ctx.alpha],
            ..default()
        });
        if let (Some(c), Some(ui)) = (ctx.cursor, ctx.cursor_ui) {
            if rect.contains(c) {
                *hover = match name {
                    DotName::Guid => MinimapBlipHover::Npc(guid, ui, grey),
                    DotName::Known(n) => MinimapBlipHover::TrackedGo(n, ui, grey),
                };
            }
        }
    };
    // Cell 0, GameObjects. No quest-status check: the GameObject leg (`0x4eab43`) goes straight
    // to `0x5ed2b0` and never reaches the status-7 compare.
    if tracking.resources != 0 {
        if let Some(locks) = locks {
            for (guid, net, tf, _) in candidates.iter() {
                if net.kind != EntityKind::GameObject {
                    continue;
                }
                let Some(t) = templates.get(guid.0) else {
                    continue; // template not answered yet
                };
                if tracked_resource(tracking.resources, t.lock_id, locks) {
                    dot(guid.0, tf, TRACKED_GO_CELL, DotName::Known(t.name.clone()));
                }
            }
        }
    }
    // Cell 1, units.
    for (guid, net, tf, store) in candidates.iter() {
        if !matches!(net.kind, EntityKind::Unit | EntityKind::Player) {
            continue;
        }
        if !unit_dot_eligible(store, self_guid) {
            continue;
        }
        if statuses.get(&guid.0).copied() == Some(7) {
            continue; // the ==7 branch already drew the quest dot
        }
        let (dyn_flags, form, creeping) = store
            .map(|s| {
                (
                    s.0.unit_dynamic_flags(),
                    s.0.unit_shapeshift_form(),
                    s.0.unit_is_stealthed(),
                )
            })
            .unwrap_or((0, 0, false));
        let creature_type = creature_type_of(
            net.kind,
            form,
            benilla_protocol::guid::entry(guid.0),
            names,
            forms,
        );
        if tracked_creature(tracking, creature_type, dyn_flags, creeping) {
            dot(guid.0, tf, TRACKED_UNIT_CELL, DotName::Guid);
        }
    }
}

/// How a tracking dot's hover names itself: a unit resolves by guid through the name cache;
/// a GameObject's template name is already in hand.
enum DotName {
    Guid,
    Known(String),
}

/// The party dots, the in-range half of the party placement `0x6dad10`: blue cell 4 at 1.3×,
/// drawn last with the object dots, above every arrow (`0x4ed7b7`).
pub(in crate::minimap) fn emit_party_dots(
    ctx: &BlipCtx,
    group: &crate::ui_party::GroupState,
    guids: &GuidIndex,
    unit_pos: &Query<&GlobalTransform, With<NetEntity>>,
    icons: &Handle<Image>,
    quads: &mut UiQuads,
) {
    for m in group.party_slots() {
        let Some((x, y)) = party_member_pos(m, group, guids, unit_pos) else {
            continue;
        };
        let d = ((x - ctx.wx).powi(2) + (y - ctx.wy).powi(2)).sqrt();
        if d / ctx.radius_yd > super::BLIP_EDGE_RATIO {
            continue; // out of range: the arrow pass drew it
        }
        quads.overlays.push(UiQuad {
            rect: Rect::from_center_size(
                ctx.center + ctx.offset([x, y, 0.0]),
                Vec2::splat(ctx.side * (PARTY_DOT_PX / BLIP_BASIS_PX)),
            ),
            z_key: ctx.z,
            texture: Some(icons.clone()),
            uv: UvRect::from_tex_coords(PARTY_DOT_CELL),
            color: [1.0, 1.0, 1.0, ctx.alpha],
            ..default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first row is the control: an ordinary live creature still passes.
    #[test]
    fn the_dead_our_own_minions_and_the_untrackable_draw_no_dot_of_either_kind() {
        use crate::net::ObjectStore;
        use benilla_protocol::ObjectFields;

        const FIELD_HEALTH: u16 = 22; // UNIT_FIELD_HEALTH
        const FIELD_SUMMONEDBY: u16 = 12; // UNIT_FIELD_SUMMONEDBY (2 dwords)
        const FIELD_CHARMEDBY: u16 = 10; // UNIT_FIELD_CHARMEDBY (2 dwords)
        const FIELD_BYTES_1: u16 = 138;
        /// `UNIT_FIELD_BYTES_1` byte 3 bit 2, the `& 4` the classifier tests.
        const UNTRACKABLE: u32 = 0x4 << 24;
        const ME: u64 = 0x0000_0000_0000_0007;
        const SOMEONE_ELSE: u64 = 0x0000_0000_0000_0042;

        let store = |pairs: &[(u16, u32)]| ObjectStore(ObjectFields::from_pairs(pairs));
        let alive = [(FIELD_HEALTH, 100u32)];
        let with = |extra: &[(u16, u32)]| {
            let mut v = alive.to_vec();
            v.extend_from_slice(extra);
            store(&v)
        };
        let lo = |g: u64| (g & 0xffff_ffff) as u32;
        let hi = |g: u64| (g >> 32) as u32;

        assert!(
            unit_dot_eligible(Some(&with(&[])), Some(ME)),
            "the control: an ordinary live creature still takes a dot"
        );
        assert!(
            !unit_dot_eligible(Some(&store(&[(FIELD_HEALTH, 0)])), Some(ME)),
            "the dead draw nothing — a SIGNED `<= 0` at 0x4eac1e"
        );
        assert!(
            !unit_dot_eligible(None, Some(ME)),
            "…and so does a candidate whose descriptor has not landed"
        );
        assert!(
            !unit_dot_eligible(
                Some(&with(&[
                    (FIELD_SUMMONEDBY, lo(ME)),
                    (FIELD_SUMMONEDBY + 1, hi(ME)),
                ])),
                Some(ME)
            ),
            "our own minion is not a blip — equality on the owner guid is the REJECT"
        );
        assert!(
            unit_dot_eligible(
                Some(&with(&[
                    (FIELD_SUMMONEDBY, lo(SOMEONE_ELSE)),
                    (FIELD_SUMMONEDBY + 1, hi(SOMEONE_ELSE)),
                ])),
                Some(ME)
            ),
            "…but somebody ELSE's minion still is"
        );
        assert!(
            !unit_dot_eligible(
                Some(&with(&[
                    (FIELD_CHARMEDBY, lo(ME)),
                    (FIELD_CHARMEDBY + 1, hi(ME)),
                    // A non-zero CHARMEDBY wins; SUMMONEDBY is only the fallback.
                    (FIELD_SUMMONEDBY, lo(SOMEONE_ELSE)),
                    (FIELD_SUMMONEDBY + 1, hi(SOMEONE_ELSE)),
                ])),
                Some(ME)
            ),
            "a unit WE charmed is not a blip, and charm outranks summon"
        );
        assert!(
            !unit_dot_eligible(Some(&with(&[(FIELD_BYTES_1, UNTRACKABLE)])), Some(ME)),
            "and the untrackable bit blanks it outright"
        );
    }

    /// Status 6 draws nothing on the 1.12 client despite vmangos's "red dot" comment (`0x4eac31`).
    #[test]
    fn quest_dot_is_status_seven_only_gold_cell_three() {
        assert_eq!(quest_dot_cell(7), Some([0.75, 1.0, 0.0, 0.25]));
        for s in [0, 1, 2, 3, 4, 5, 6, 8] {
            assert_eq!(quest_dot_cell(s), None, "status {s} must not dot");
        }
    }

    /// Bit `1 << (n − 1)`, `n` the lock's skill-slot `LockType` or the creature type.
    #[test]
    fn tracking_predicates_follow_the_mask_bit_law() {
        use benilla_formats::{LockSlot, LOCK_KEY_ITEM, MAX_LOCK_SLOTS};
        // A Mining (LockType 3) skill slot, and a key-item slot with the same index.
        let mut vein = [LockSlot::default(); MAX_LOCK_SLOTS];
        vein[0] = LockSlot {
            key_type: LOCK_KEY_SKILL,
            index: 3,
            skill: 0,
            action: 0,
        };
        let mut keyed = [LockSlot::default(); MAX_LOCK_SLOTS];
        keyed[0] = LockSlot {
            key_type: LOCK_KEY_ITEM,
            index: 3,
            skill: 0,
            action: 0,
        };
        let locks = LockCatalog::from_rows([(38, vein), (40, keyed)]);
        assert!(tracked_resource(1 << 2, 38, &locks), "mining bit lights it");
        assert!(
            !tracked_resource(1 << 1, 38, &locks),
            "herbalism bit doesn't"
        );
        assert!(!tracked_resource(0, 38, &locks), "no mask, no dot");
        assert!(
            !tracked_resource(1 << 2, 0, &locks),
            "lockId 0 never tracks"
        );
        assert!(
            !tracked_resource(1 << 2, 40, &locks),
            "a key-ITEM slot's index is an item entry, not a LockType"
        );
        assert!(!tracked_resource(1 << 2, 99, &locks), "unknown lock id");

        // Track Beasts sets bit 0 (Beast is creature type 1).
        let beasts = SelfTracking {
            creatures: 1,
            ..default()
        };
        assert!(tracked_creature(beasts, Some(1), 0, false));
        assert!(!tracked_creature(
            beasts,
            Some(CREATURE_TYPE_HUMANOID),
            0,
            false
        ));
        assert!(!tracked_creature(
            SelfTracking::default(),
            Some(1),
            0,
            false
        ));
        assert!(
            !tracked_creature(beasts, None, 0, false),
            "type not cached yet — no dot"
        );
        // The always-show pair: Hunter's Mark needs no tracking aura; track-stealthed needs
        // both our bit and the unit's CREEP flag.
        assert!(tracked_creature(
            SelfTracking::default(),
            None,
            UNIT_DYNFLAG_TRACK_UNIT,
            false
        ));
        let stealth = SelfTracking {
            stealthed: true,
            ..default()
        };
        assert!(tracked_creature(stealth, None, 0, true));
        assert!(!tracked_creature(stealth, None, 0, false), "not sneaking");
        assert!(
            !tracked_creature(SelfTracking::default(), None, 0, true),
            "we don't track stealthed"
        );
    }

    #[test]
    fn creature_type_resolver_prefers_the_shapeshift_override() {
        use benilla_formats::ShapeshiftForm;
        let names = NameCache::default();
        let forms: HashMap<u32, ShapeshiftForm> = [
            (
                1,
                ShapeshiftForm {
                    creature_type: 1, // Cat → Beast
                    ..Default::default()
                },
            ),
            (
                16,
                ShapeshiftForm {
                    creature_type: 0, // a <=0 row reads Humanoid (the resolver's fallback)
                    ..Default::default()
                },
            ),
        ]
        .into();
        // A cat-form player is a Beast; unshifted, a Humanoid.
        assert_eq!(
            creature_type_of(EntityKind::Player, 1, None, &names, Some(&forms)),
            Some(1)
        );
        assert_eq!(
            creature_type_of(EntityKind::Player, 0, None, &names, Some(&forms)),
            Some(CREATURE_TYPE_HUMANOID)
        );
        assert_eq!(
            creature_type_of(EntityKind::Player, 16, None, &names, Some(&forms)),
            Some(CREATURE_TYPE_HUMANOID)
        );
        // An NPC with no cached template yet resolves nothing.
        assert_eq!(
            creature_type_of(EntityKind::Unit, 0, Some(69), &names, Some(&forms)),
            None
        );
        assert_eq!(
            creature_type_of(EntityKind::GameObject, 0, None, &names, Some(&forms)),
            None
        );
    }

    /// The tracking spell's `EffectMiscValue`, the server's mask bit and the node's `Lock.dbc`
    /// skill slot, on the install's data.
    #[test]
    fn real_find_minerals_lights_a_copper_vein_not_an_herb() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let spells = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let locks = benilla_formats::load_lock_catalog(&mut chain).expect("Lock.dbc");
        let forms =
            benilla_formats::load_shapeshift_forms(&mut chain).expect("SpellShapeshiftForm.dbc");

        // Bit `1 << (MiscValue − 1)` of the aura-`kind` effect (44 creatures, 45 resources).
        let mask_of = |spell_id: u32, kind: u32| -> u32 {
            let s = spells.get(spell_id).expect("spell row");
            (0..3)
                .find_map(|i| {
                    (s.effect_apply_aura[i] == kind).then(|| {
                        let m = s.effect_misc_value[i];
                        assert!((1..=32).contains(&m), "MiscValue {m} out of mask range");
                        1u32 << (m - 1)
                    })
                })
                .expect("tracking effect present")
        };

        // Find Minerals 2580 and a Copper Vein (vmangos `gameobject_template` 1731, lockId 38).
        let minerals = mask_of(2580, 45);
        assert_eq!(minerals, 1 << 2, "Find Minerals' MiscValue is Mining (3)");
        assert!(tracked_resource(minerals, 38, &locks));
        // Find Herbs 2383 and Peacebloom or Silverleaf (lockId 29, Herbalism LockType 2).
        let herbs = mask_of(2383, 45);
        assert!(tracked_resource(herbs, 29, &locks));
        assert!(!tracked_resource(minerals, 29, &locks), "cross-profession");
        assert!(!tracked_resource(herbs, 38, &locks), "cross-profession");
        // Track Beasts 1494 (MiscValue 1, Beast) lights a cat-form (1) druid.
        let beasts = SelfTracking {
            creatures: mask_of(1494, 44),
            ..Default::default()
        };
        assert!(tracked_creature(beasts, Some(1), 0, false));
        assert!(!tracked_creature(
            beasts,
            Some(CREATURE_TYPE_HUMANOID),
            0,
            false
        ));
        let cat = creature_type_of(
            EntityKind::Player,
            1,
            None,
            &NameCache::default(),
            Some(&forms),
        );
        assert_eq!(cat, Some(1), "cat form resolves Beast (DBC col 12)");
        assert!(tracked_creature(beasts, cat, 0, false));
    }
}
