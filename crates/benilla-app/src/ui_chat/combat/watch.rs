//! The combat-log lines that are **not packet-driven at all** — the reference's two families of
//! them, death and aura, plus the pet-loyalty leaf.
//!
//! Everything else in [`super`] hangs off an SMSG arm. These three hang off *descriptor changes*,
//! exactly as the reference does:
//!
//! - **the death line** (`UNITDIES*` / `UNITDESTROYEDOTHER`) rides the unit-death reflex
//!   `0x625190`, not a packet — there is no "X died" opcode in 1.12;
//! - **the aura lines** (`AURAADDED*`, `AURAREMOVED*`, `AURAAPPLICATIONADDED*`) ride the
//!   `UNIT_FIELD_AURA` / `UNIT_FIELD_AURAAPPLICATIONS` change callbacks `0x604d00` / `0x604ea0`
//!   through the bridges `0x612320`/`0x6123f0`/`0x612450`;
//! - **the loyalty line** rides the `UNIT_FIELD_BYTES_1` byte-1 callback (`0x5ff860` → `0x62d440`).
//!
//! Each is a slot diff against the last frame's snapshot, the same idiom
//! [`crate::creature_anim::arm_aura_state_fx`] uses for the aura state kits — and for the same
//! reason: the *edge* is the event, and a descriptor snapshot is all the wire gives us.

use bevy::prelude::*;

use super::{
    aura_gone_kind, classify, death_kind, in_range, periodic_kind, CombatLogRanges, Family, Fills,
    Named, PendingCombat, UnitClass, Variant,
};
use crate::net::{FieldChanged, GuidIndex, ObjectStore, Reputations, SelfGuid};
use crate::target::ring::Factions;
use crate::ui_chat::{ChatEventKind, ChatLog};
use crate::ui_party::GroupState;

/// The classification inputs all three watchers share — the same six facts
/// [`crate::combat_log::chat::ChatCtx`] carries, as a `SystemParam` so a watcher's own
/// parameter list stays about *its* diff.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct WatchCtx<'w> {
    pub self_guid: Res<'w, SelfGuid>,
    pub group: Res<'w, GroupState>,
    pub index: Res<'w, GuidIndex>,
    pub factions: Option<Res<'w, Factions>>,
    pub reputations: Res<'w, Reputations>,
    pub spells: Option<Res<'w, crate::ui_action::Spells>>,
    /// The live display ranges — the reference's `0x8629e0` table plus `CombatDeathLogRange`.
    pub ranges: Res<'w, CombatLogRanges>,
}

impl WatchCtx<'_> {
    fn classify(&self, guid: u64, stores: &Query<(Entity, &ObjectStore)>) -> UnitClass {
        classify(
            guid,
            &self.self_guid,
            Some(&self.group),
            &self.index,
            stores,
            self.factions.as_deref(),
            &self.reputations,
        )
    }

    /// [`crate::combat_log::chat::ChatCtx::spell_name`]'s law, shared: the per-spell gates
    /// (`Attributes & 0x180`, an empty localized name) apply to every spell-driven line, and an
    /// aura line is one.
    fn spell_name(&self, spell_id: u32) -> Option<String> {
        let Some(display) = self.spells.as_ref().and_then(|s| s.catalog.get(spell_id)) else {
            return Some(String::new());
        };
        const DO_NOT_DISPLAY_OR_LOG: u32 = 0x180;
        if display.attributes & DO_NOT_DISPLAY_OR_LOG != 0 || display.name.is_empty() {
            return None;
        }
        Some(display.name.clone())
    }

    /// The reference's ONE-SIDED range test — the shape 15 formatters run instead of the two-ended
    /// gate `0x626630`, the death and aura lines among them. One participant, one distance.
    ///
    /// **`range` is the caller's**, because the death line does not use the class table: its
    /// formatter `0x62c160` reads `CombatDeathLogRange` first (`0x62c19c`) and only falls back to
    /// the per-class getter when that *lookup* fails — which never happens in a client that
    /// registered it at startup. The aura lines have no CVar of their own and stay per-class.
    fn in_range(&self, guid: u64, range: f32, poses: &Query<&Transform>) -> bool {
        in_range(guid, range, &self.self_guid, &self.index, poses)
    }
}

/// Queue one single-endpoint line, gated the way its formatter gates.
fn queue_one(
    log: &mut ChatLog,
    ctx: &WatchCtx,
    poses: &Query<&Transform>,
    kind: ChatEventKind,
    family: Family,
    subject: (u64, UnitClass),
    range: f32,
    fills: Fills,
    named: Named,
) {
    if !ctx.in_range(subject.0, range, poses) {
        return;
    }
    log.push_combat(PendingCombat {
        kind,
        family,
        variant: Variant::of(subject.1, UnitClass::Creature),
        subject: subject.0,
        object: 0,
        fills,
        named,
        tries: 0,
    });
}

/// `Spell.dbc` `Effect[0]` values that make a unit "destroyed" rather than "dead" — `0x62c320`'s
/// byte table, read through `UNIT_CREATED_BY_SPELL`. A totem, a summoned object, a guardian: the
/// things the world *made*, which are unmade rather than killed.
const DESTROYED_EFFECTS: [u32; 10] = [50, 74, 87, 88, 89, 90, 104, 105, 106, 107];

/// The unit-death reflex's chat line (`0x625190` → `0x62c160`).
///
/// **The edge is the event.** The wire never says "this unit died"; it says the unit's health is
/// zero in an ordinary descriptor update — so the line comes off the `UNIT_FIELD_HEALTH` field
/// edge, the reference's own watcher `0x6046f0` whose alive→dead arm (`OLD > 0 && NEW ≤ 0` at
/// `0x6047a3`) reaches the reflex (decision 2297), and a unit that streams in *already* dead
/// produces none (the edge stream is create-suppressed: the reflex fires on the transition, not
/// on the state).
///
/// **The XP-award suppression is deliberately not modelled**, and is named rather than dropped:
/// `[CGUnit+0xc58] bit 3`, set only in the `SMSG_LOG_XPGAIN` chain, makes the death reflex emit the
/// award line *instead of* the plain death line (`0x62c23e`). No clearing site for the bit is
/// known, so whether it survives a second death is UNSETTLED — and modelling an unsettled latch is
/// how you get a line that silently stops appearing. The visible consequence today is one extra
/// "%s dies." beside the XP line on a kill that awards experience.
pub(crate) fn death_lines(
    ctx: WatchCtx,
    stores: Query<(Entity, &ObjectStore)>,
    mut edges: MessageReader<FieldChanged>,
    poses: Query<&Transform>,
    mut log: ResMut<ChatLog>,
) {
    for e in edges.read() {
        if !(e.unit_field(benilla_protocol::field::FIELD_UNIT_HEALTH) && e.old > 0 && e.new == 0) {
            continue;
        }
        let Ok((_, store)) = stores.get(e.entity) else {
            continue;
        };
        let guid = e.guid;
        let class = ctx.classify(guid, &stores);
        // A summoned thing is *destroyed*; a living one *dies*. The test is the creating spell's
        // first effect, which is why it needs the spell catalog at all.
        let destroyed = store
            .0
            .unit_created_by_spell()
            .filter(|&id| id != 0)
            .and_then(|id| ctx.spells.as_ref()?.catalog.get(id))
            .is_some_and(|d| DESTROYED_EFFECTS.contains(&d.effects[0]));
        let family = match (class, destroyed) {
            (UnitClass::Me, _) => super::UNITDIES,
            (_, true) => super::UNITDESTROYEDOTHER,
            (_, false) => super::UNITDIES,
        };
        queue_one(
            &mut log,
            &ctx,
            &poses,
            death_kind(class),
            family,
            (guid, class),
            // The death line's own CVar, not the class table — the one formatter that has one.
            ctx.ranges.death(),
            Fills::default(),
            Named::Ready,
        );
    }
}

/// The aura lines (`0x62b480` / `0x62b800`).
///
/// **Three edges out of two watches**, because the reference reads two descriptor arrays through
/// two callbacks and words three sentences — and since decision 2297 those are literally the
/// triggers here, one field edge each, with no snapshot of the slots kept anywhere:
///
/// - a `UNIT_FIELD_AURA` slot whose NEW value is a spell → `AURAADDED{SELF,OTHER}{HARMFUL,HELPFUL}`,
///   at the PERIODIC chat types (harmful takes the `…_DAMAGE` row, helpful the `…_BUFFS` one) —
///   not the `AURA_GONE` block, which is the departure's alone (`0x604d00`'s add arm);
/// - a slot whose OLD value was a spell → `AURAREMOVED{SELF,OTHER}`, at `0x41`/`0x42`/`0x43`
///   (the remove arm; a replace is one of each, the arrival worded first);
/// - a `UNIT_FIELD_AURAAPPLICATIONS` byte that ROSE, on a slot whose spell did not move this
///   frame → `AURAAPPLICATIONADDED*` (`0x604ea0`), and only when the spell can stack at all. We
///   approximate the reference's `SpellRec->StackAmount > 1` gate with the observed count: a
///   stack that rose past one is a spell that stacks. It differs only for a first application of
///   a stacking aura, which the reference words as a plain `AURAADDED*` — and so do we, because a
///   fresh slot's count is not a rise (its `AURA` dword moved in the same block, so its byte is
///   skipped here).
///
/// **HARMFUL is the SLOT INDEX, not a flag** (three byte sites, e.g. `0x61238e`): slots
/// `0x20`–`0x2f` are harmful, every other slot helpful. `UNIT_FIELD_AURAFLAGS` is only an occupancy
/// predicate — using it here would be the wrong mechanism that happens to agree most of the time.
pub(crate) fn aura_lines(
    ctx: WatchCtx,
    stores: Query<(Entity, &ObjectStore)>,
    mut edges: MessageReader<FieldChanged>,
    poses: Query<&Transform>,
    mut log: ResMut<ChatLog>,
) {
    use benilla_protocol::field::{FIELD_UNIT_AURA, FIELD_UNIT_AURAAPPLICATIONS};
    /// The first harmful aura slot — `0x20`. Slots below it are helpful (`0x61238e`).
    const FIRST_HARMFUL_SLOT: u16 = 0x20;
    const SLOTS: u16 = benilla_protocol::messages::UNIT_AURA_SLOTS as u16;
    let harmful = |slot: u16| slot >= FIRST_HARMFUL_SLOT;

    // The whole frame's batch first: the applications leg must know which slots' `AURA` dword
    // moved in the same block, and the two arrays sit 66 dwords apart in the ascending walk.
    let batch: Vec<FieldChanged> = edges.read().copied().collect();
    let slot_edges: Vec<&FieldChanged> = batch
        .iter()
        .filter(|e| e.unit_array_slot(FIELD_UNIT_AURA, SLOTS).is_some())
        .collect();
    let slot_moved = |entity: Entity, slot: u16| {
        slot_edges
            .iter()
            .any(|e| e.entity == entity && e.index - FIELD_UNIT_AURA == slot)
    };

    for e in &slot_edges {
        let slot = e.index - FIELD_UNIT_AURA;
        let class = ctx.classify(e.guid, &stores);
        // Arrival first: the slot now holds a spell (a fill, or a replace's new half).
        if e.new != 0 {
            if let (Some(spell), Some(kind)) =
                (ctx.spell_name(e.new), periodic_kind(class, !harmful(slot)))
            {
                queue_one(
                    &mut log,
                    &ctx,
                    &poses,
                    kind,
                    if harmful(slot) {
                        super::AURAADDED_HARMFUL
                    } else {
                        super::AURAADDED_HELPFUL
                    },
                    (e.guid, class),
                    ctx.ranges.class(class),
                    Fills {
                        spell,
                        ..Default::default()
                    },
                    Named::Ready,
                );
            }
        }
        // Departure: the slot held a spell and no longer holds THAT spell.
        if e.old != 0 {
            if let Some(spell) = ctx.spell_name(e.old) {
                queue_one(
                    &mut log,
                    &ctx,
                    &poses,
                    aura_gone_kind(class),
                    super::AURAREMOVED,
                    (e.guid, class),
                    ctx.ranges.class(class),
                    Fills {
                        spell,
                        ..Default::default()
                    },
                    Named::Ready,
                );
            }
        }
    }

    // The stack-count leg: byte-packed, four slots per dword, the wire byte `stack - 1`.
    for e in batch.iter().filter(|e| {
        e.unit_array_slot(FIELD_UNIT_AURAAPPLICATIONS, SLOTS / 4)
            .is_some()
    }) {
        let word = e.index - FIELD_UNIT_AURAAPPLICATIONS;
        let Ok((_, store)) = stores.get(e.entity) else {
            continue; // gone in the same drain
        };
        for byte in 0..4u16 {
            let slot = word * 4 + byte;
            let (before, now) = ((e.old >> (byte * 8)) & 0xff, (e.new >> (byte * 8)) & 0xff);
            // A rise past one, on a slot whose spell stayed put this block.
            if now <= before || now == 0 || slot_moved(e.entity, slot) {
                continue;
            }
            let Some(aura) = store.0.unit_aura(slot as u8) else {
                continue;
            };
            let Some(spell) = ctx.spell_name(aura.spell_id) else {
                continue;
            };
            let class = ctx.classify(e.guid, &stores);
            let Some(kind) = periodic_kind(class, !harmful(slot)) else {
                continue;
            };
            queue_one(
                &mut log,
                &ctx,
                &poses,
                kind,
                if harmful(slot) {
                    super::AURAAPPLICATIONADDED_HARMFUL
                } else {
                    super::AURAAPPLICATIONADDED_HELPFUL
                },
                (e.guid, class),
                ctx.ranges.class(class),
                Fills {
                    spell,
                    amount: i64::from(now + 1),
                    ..Default::default()
                },
                Named::Ready,
            );
        }
    }
}

/// The pet-loyalty line (`0x5ff860` → `0x62d440`) — `UNIT_FIELD_BYTES_1` byte 1 moving.
///
/// **Both legs require the pet's owner to be you**, which is the reference's own guard, and the
/// chat type is the literal `0x19` `COMBAT_MISC_INFO`. The reference emits the localized text as
/// the single `%s` argument of a bare `"%s"`; a no-slot family reaches the same sentence.
pub(crate) fn pet_loyalty_lines(
    ctx: WatchCtx,
    stores: Query<(Entity, &ObjectStore)>,
    mut edges: MessageReader<FieldChanged>,
    mut log: ResMut<ChatLog>,
) {
    for e in edges.read() {
        if !e.unit_field(benilla_protocol::field::FIELD_UNIT_BYTES_1) {
            continue;
        }
        // Byte 1 of the word — the same shift `ObjectFields::unit_loyalty_level` reads through.
        let (before, level) = (((e.old >> 8) & 0xff) as u8, ((e.new >> 8) & 0xff) as u8);
        if level == before || level == 0 || before == 0 {
            continue;
        }
        if ctx.classify(e.guid, &stores) != UnitClass::MyPet {
            continue;
        }
        log.push_combat(PendingCombat {
            kind: ChatEventKind::CombatMiscInfo,
            family: if level > before {
                super::PET_LOYALTY_GAIN
            } else {
                super::PET_LOYALTY_LOSS
            },
            variant: Variant::OtherOther,
            subject: 0,
            object: 0,
            fills: Fills::default(),
            named: Named::Ready,
            tries: 0,
        });
    }
}
