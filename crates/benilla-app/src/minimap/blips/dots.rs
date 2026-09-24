//! The `ObjectIcons.blp` **dot layer** — the per-object cell lists of the classifier
//! `0x4eaa90` (quest gold cell 3, tracking gold/red cells 0/1, party blue cell 4). Split from
//! the blip layer's landmark/arrow half; the shared frame geometry ([`BlipCtx`]), hover slot,
//! and size basis live in the parent module (the byte law and its provenance are in the
//! parent's module doc).

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

/// The quest dot's quad: 8 × 8 px (`bc82a8 = base·0.00625` × 1280, ctor-frozen; the per-cell
/// scale table `{1,1,1,1,1.3}` leaves quest cell 3 at 1.0 — only the party cell 4 is 1.3×).
const QUEST_DOT_PX: f32 = 8.0;
/// The party dot's quad: the same 8-px base at the cell table's 1.3× party scale.
const PARTY_DOT_PX: f32 = 8.0 * 1.3;
/// `ObjectIcons.blp` cell 4 — the blue party-member dot (col 0, row 1 of the 4×4 grid).
const PARTY_DOT_CELL: [f32; 4] = [0.0, 0.25, 0.25, 0.5];
/// `ObjectIcons.blp` cell 0 — the gold tracked-RESOURCE dot (col 0, row 0): a GameObject
/// passing the resource-tracking predicate (Find Herbs/Minerals; `0x5ed2b0`).
const TRACKED_GO_CELL: [f32; 4] = [0.0, 0.25, 0.0, 0.25];
/// `ObjectIcons.blp` cell 1 — the red tracked-UNIT dot (col 1, row 0): a unit passing the
/// creature-tracking predicate (Track Beasts/Humanoids/…; `0x5ed210`).
const TRACKED_UNIT_CELL: [f32; 4] = [0.25, 0.5, 0.0, 0.25];
/// `UNIT_DYNAMIC_FLAGS` bit 0x2 — the per-viewer "always show on minimap" flag (vmangos
/// `UNIT_DYNFLAG_TRACK_UNIT`; the server sets it on a Hunter's Mark victim for the caster).
/// Byte-verified as `0x5ed210`'s first clause (`+0x224 & 0x2`, the 0564 fold-back).
const UNIT_DYNFLAG_TRACK_UNIT: u32 = 0x2;
/// Creature type 7 — Humanoid: the resolver's player/race fallback. Byte-verified via the
/// shipped `ChrRaces.dbc` (col 9 = 7 for all nine playable races, read by `0x605570`; the 0564
/// fold-back) — also the `<= 0` fallback of the shapeshift override.
const CREATURE_TYPE_HUMANOID: u32 = 7;

/// `ObjectIcons.blp` cell for a DIALOG_STATUS — **status 7 only** (`cmp [obj+0xcb8],7` at
/// `0x4eac31`, VERIFIED): the gold cell 3. Every other status draws no quest dot.
fn quest_dot_cell(status: u32) -> Option<[f32; 4]> {
    (status == 7).then_some([0.75, 1.0, 0.0, 0.25])
}

/// The three preconditions `0x4eaa90` applies to a **UNIT or PLAYER** before either dot category
/// is even chosen (decision 1906). They sit upstream of the `cmp [edi+0xcb8],7` at `0x4eac31`,
/// whose only predecessor is the fall-through, so they gate the gold **quest** dot (cell 3) and the
/// red **tracking** dot (cell 1) alike. They do **not** touch the GameObject leg (cell 0) or the
/// party dots (cell 4), which are reached by other paths entirely.
///
/// A candidate with no descriptor yet fails, which is the same answer the reference's own read of
/// an un-streamed unit would give (a zeroed health field is `<= 0`) — and it lasts one drain, since
/// `net/apply` seeds the store at the tail of the drain that spawned the entity.
fn unit_dot_eligible(store: Option<&ObjectStore>, me: Option<u64>) -> bool {
    let Some(f) = store.map(|s| &s.0) else {
        return false;
    };
    // 1. `0x4eac19 mov ecx,[eax+0x40]; test ecx,ecx; 0x4eac1e 0f 8e jle` — a **signed** `<= 0` on
    //    `UNIT_FIELD_HEALTH` (`0f 8e`, not `0f 86`). The dead get no dot of any kind.
    if f.unit_health().unwrap_or(0) as i32 <= 0 {
        return false;
    }
    // 2. `UNIT_FIELD_CHARMEDBY` when non-zero, else `UNIT_FIELD_SUMMONEDBY`, both halves compared
    //    against the active player's guid (`0x468550`) — and **equality is the reject**: your own
    //    pet, minion or charmed victim is never a blip. (This is not the classifier's self-GUID
    //    check; there are two other, separate ones.)
    let owner = match f.unit_charmed_by() {
        Some(g) if g != 0 => g,
        _ => f.unit_summoned_by().unwrap_or(0),
    };
    if owner != 0 && Some(owner) == me {
        return false;
    }
    // 3. `byte [eax+0x213] & 4` — `UNIT_FIELD_BYTES_1` byte 3 bit 2, the only `& 4` site on that
    //    byte image-wide. The bit is verified; its *name* is vmangos's.
    !f.unit_is_untrackable()
}

/// Draw a gold dot per quest-giver at status 7, at the unit's live position, hard-culled at
/// the view radius in 3-D world distance (`range² < dx²+dy²+dz²` skips — no rim ride);
/// records a hover hit with the guid. Called AFTER the player arrow: dots draw last, on top.
///
/// Walks the candidate set rather than the status map — the classifier's own shape (it is a
/// per-object callback, not a per-status one), and the only way to reach each object's descriptor,
/// which [`unit_dot_eligible`] needs.
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
            continue; // the GameObject leg (`0x4eab43`) never reaches the `== 7` compare
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
        // The cross-interior GREY (`0xffb0b0b0`, byte-pinned render value): the classifier
        // greys entries whose colorFlag is set — the "indoor/subzone distinction" via the
        // containment query `0x670540`. Implemented as the indoor-containment MISMATCH (the
        // same down-ray the entity light classifier uses); the exact compare is INTERIM
        // pending its scoped pin.
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

/// The resource-tracking predicate (the classifier's `0x5ed2b0` leg): does this GameObject's
/// lock match the active `PLAYER_TRACK_RESOURCES` mask? The GO's template lockId resolves
/// through `Lock.dbc`; ANY skill-keyed slot whose `LockType.dbc` id `n` has mask bit
/// `1 << (n − 1)` set matches (the server sets exactly that bit from the tracking aura's
/// MiscValue — vmangos `HandleAuraTrackResources`). Lock-less GOs (lockId 0) never track.
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

/// Our own tracking state, read once off the SelfPlayer descriptor: the two masks + the
/// track-stealthed bit (`PLAYER_FIELD_BYTES & 0x2`).
#[derive(Clone, Copy, Default)]
pub(in crate::minimap) struct SelfTracking {
    pub(in crate::minimap) creatures: u32,
    pub(in crate::minimap) resources: u32,
    pub(in crate::minimap) stealthed: bool,
}

/// The creature-tracking predicate — `0x5ed210` (the 0564 fold-back): two always-show clauses
/// first — `UNIT_DYNFLAG_TRACK_UNIT` (Hunter's Mark) and *our* track-stealthed bit against the
/// target's CREEP vis-flag (the TRACK_STEALTHED(151) consumer) — then the unit's creature type
/// against `PLAYER_TRACK_CREATURES` (bit `creatureType − 1`). No alive/dead or faction gate
/// (byte-verified: neither predicate tests either).
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

/// The client's creature-type resolver, transcribed — `0x605570` (3-way, the 0564
/// fold-back): a nonzero shapeshift form reads `SpellShapeshiftForm.dbc`'s creatureType FIRST
/// (`<= 0` → Humanoid — a cat-form druid is a Beast); else an NPC reads its cached creature
/// template, a player its race → Humanoid (`ChrRaces.dbc` col 9 = 7 for all nine playable
/// races, dumped from the shipped file).
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

/// Draw the tracking dots (decision 0560): the gold cell-0 dot per tracked GameObject, then
/// the red cell-1 dot per tracked unit — the classifier's fall-through for objects NOT at
/// quest status 7 (those draw the quest dot instead; the `==7` branch is tested first,
/// byte-verified `0x4eac31`). Same hard 3-D radius cull, cross-interior grey, and hover law
/// as the quest dots; drawn just before them (the draw walks the cell lists in order, so
/// cells 0/1 sit under a same-spot quest or party dot).
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
    // NB the unit pass runs even with an empty creature mask — the two always-show clauses
    // need no tracking aura bit in it (Hunter's Mark marks the victim, not the caster;
    // track-stealthed rides PLAYER_FIELD_BYTES, not the mask).
    // A candidate's dot, shared by both passes: range-cull, grey, push, hover.
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
    // Cell 0 — tracked GameObjects (gold): template lockId through Lock.dbc.
    //
    // **No quest-status precedence on this leg** (decision 1872). This loop used to skip a
    // GameObject whose status was 7, mirroring the unit loop below — but the classifier's
    // `cmp dword ptr [edi+0xcb8],7` at `0x4eac31` is reachable **only** from the UNIT and PLAYER
    // legs (machine-enumerated predecessors): the GameObject leg at `0x4eab43` falls straight
    // into `0x5ed2b0`, GameObject *tracking*, and emits category 0. A GameObject never draws a
    // quest dot in the reference, so nothing about a quest status may suppress its tracking dot —
    // and a GameObject can no longer *hold* a status here anyway (`net/apply` drops it, as
    // `0x5dc9f0` does).
    if tracking.resources != 0 {
        if let Some(locks) = locks {
            for (guid, net, tf, _) in candidates.iter() {
                if net.kind != EntityKind::GameObject {
                    continue;
                }
                let Some(t) = templates.get(guid.0) else {
                    continue; // template not answered yet — no lock to test
                };
                if tracked_resource(tracking.resources, t.lock_id, locks) {
                    dot(guid.0, tf, TRACKED_GO_CELL, DotName::Known(t.name.clone()));
                }
            }
        }
    }
    // Cell 1 — tracked units (red): the 3-way creature-type resolver + the always-show pair.
    for (guid, net, tf, store) in candidates.iter() {
        if !matches!(net.kind, EntityKind::Unit | EntityKind::Player) {
            continue;
        }
        // The same three preconditions the quest dot passes — they are upstream of the branch
        // that chooses between the two categories (`0x4eac31`), so neither category outruns them.
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

/// The party **dots** — the in-range half of `place_party_raid_blips`: the blue `ObjectIcons`
/// cell 4 at the member's true position, at the cell table's 1.3× scale (10.4 px on the frozen
/// basis). Drawn last with the object dots (`0x4ed7b7`'s order: above the arrows and the player
/// arrow).
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
            continue; // out of range — the arrow pass drew it
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

    /// **The three preconditions upstream of BOTH dot categories** (decision 1906). They live in
    /// `0x4eaa90` between the type gate and the `cmp [edi+0xcb8],7`, whose only predecessor is
    /// the fall-through — so a unit that fails any of them draws neither the gold quest dot nor
    /// the red tracking dot. benilla had none of them.
    ///
    /// The control is the first row: an ordinary live creature still passes, which is what would
    /// catch a predicate written one bit too wide and blanked the minimap.
    #[test]
    fn the_dead_our_own_minions_and_the_untrackable_draw_no_dot_of_either_kind() {
        use crate::net::ObjectStore;
        use benilla_protocol::ObjectFields;

        const FIELD_HEALTH: u16 = 22; // UNIT_FIELD_HEALTH
        const FIELD_SUMMONEDBY: u16 = 12; // UNIT_FIELD_SUMMONEDBY (2 dwords)
        const FIELD_CHARMEDBY: u16 = 10; // UNIT_FIELD_CHARMEDBY (2 dwords)
        const FIELD_BYTES_1: u16 = 138;
        /// `UNIT_FIELD_BYTES_1` byte 3 bit 2 — the `& 4` the classifier tests.
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
                    // A non-zero CHARMEDBY wins outright — SUMMONEDBY is only the fallback.
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

    /// Only status 7 dots (the gold cell 3). Status 6 — despite the vmangos "red dot"
    /// comment — draws nothing on the 1.12 client (byte-verified `==7` at 0x4eac31).
    #[test]
    fn quest_dot_is_status_seven_only_gold_cell_three() {
        assert_eq!(quest_dot_cell(7), Some([0.75, 1.0, 0.0, 0.25]));
        for s in [0, 1, 2, 3, 4, 5, 6, 8] {
            assert_eq!(quest_dot_cell(s), None, "status {s} must not dot");
        }
    }

    /// The tracking predicates' mask-bit law (decision 0560): bit `1 << (n − 1)` where `n` is
    /// the GO lock's skill-slot `LockType` id (resources) or the unit's creature type
    /// (creatures) — the exact bit the server sets from the tracking aura's MiscValue — plus
    /// the always-show dyn-flag clause that needs no mask at all.
    #[test]
    fn tracking_predicates_follow_the_mask_bit_law() {
        use benilla_formats::{LockSlot, LOCK_KEY_ITEM, MAX_LOCK_SLOTS};
        // A mining-vein-shaped lock (one SKILL slot, Mining LockType 3) and a key-ITEM lock
        // whose index happens to collide numerically.
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
        // The always-show pair (`0x5ed210`): Hunter's Mark forces the dot with no
        // tracking aura on us; track-stealthed lights only a CREEP-flagged unit — and only
        // the conjunction of the two bits does.
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

    /// The creature-type resolver's 3-way (`0x605570`, the 0564 fold-back): shapeshift
    /// override first (`<= 0` → Humanoid), then the cached template for NPCs, the Humanoid
    /// fallback for players.
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
        // A cat-form PLAYER is a Beast; unshifted, a player is a Humanoid.
        assert_eq!(
            creature_type_of(EntityKind::Player, 1, None, &names, Some(&forms)),
            Some(1)
        );
        assert_eq!(
            creature_type_of(EntityKind::Player, 0, None, &names, Some(&forms)),
            Some(CREATURE_TYPE_HUMANOID)
        );
        // A <=0 creatureType row resolves Humanoid, not the race/template path.
        assert_eq!(
            creature_type_of(EntityKind::Player, 16, None, &names, Some(&forms)),
            Some(CREATURE_TYPE_HUMANOID)
        );
        // An unshifted NPC with no cached template yet resolves nothing (no dot until the
        // ask-once query answers); a GameObject never resolves a creature type.
        assert_eq!(
            creature_type_of(EntityKind::Unit, 0, Some(69), &names, Some(&forms)),
            None
        );
        assert_eq!(
            creature_type_of(EntityKind::GameObject, 0, None, &names, Some(&forms)),
            None
        );
    }

    /// The whole client-side chain against the REAL 5875 data (skips without it): the
    /// tracking spell's `EffectMiscValue` → the server's mask bit → the gathering node's
    /// `Lock.dbc` skill slot. Find Minerals lights a Copper Vein, Find Herbs a Peacebloom —
    /// and neither lights the other's node.
    #[test]
    fn real_find_minerals_lights_a_copper_vein_not_an_herb() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let spells = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let locks = benilla_formats::load_lock_catalog(&mut chain).expect("Lock.dbc");
        let forms =
            benilla_formats::load_shapeshift_forms(&mut chain).expect("SpellShapeshiftForm.dbc");

        // The server's mask law, applied to the real spell row: bit `1 << (MiscValue − 1)` of
        // the aura-`kind` effect (44 TRACK_CREATURES / 45 TRACK_RESOURCES).
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

        // Find Minerals 2580 ↔ Copper Vein (gameobject_template 1731, chest lockId 38 —
        // vmangos world DB — whose Lock.dbc row is the Mining LockType 3 skill slot).
        let minerals = mask_of(2580, 45);
        assert_eq!(minerals, 1 << 2, "Find Minerals' MiscValue is Mining (3)");
        assert!(tracked_resource(minerals, 38, &locks));
        // Find Herbs 2383 ↔ Peacebloom/Silverleaf (lockId 29, Herbalism LockType 2).
        let herbs = mask_of(2383, 45);
        assert!(tracked_resource(herbs, 29, &locks));
        assert!(!tracked_resource(minerals, 29, &locks), "cross-profession");
        assert!(!tracked_resource(herbs, 38, &locks), "cross-profession");
        // Track Beasts 1494: TRACK_CREATURES MiscValue 1 = Beast — a wolf dots red, a
        // humanoid doesn't. And through the shapeshift override on the REAL
        // SpellShapeshiftForm.dbc, a cat-form (1) druid IS a Beast to it.
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
