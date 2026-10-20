//! The combat-log lines that come off descriptor edges, not packets, as in the reference: the
//! death line (the unit-death reflex `0x625190`; 1.12 has no death opcode), the aura lines (the
//! `UNIT_FIELD_AURA` and `UNIT_FIELD_AURAAPPLICATIONS` callbacks `0x604d00` and `0x604ea0`,
//! through `0x612320`, `0x6123f0` and `0x612450`) and the pet-loyalty line (`0x5ff860`).

use bevy::prelude::*;

use super::{
    aura_gone_kind, classify, death_kind, in_range, periodic_kind, CombatLogRanges, Family, Fills,
    LineWord, Named, PendingCombat, UnitClass, Variant,
};
use crate::net::{FieldChanged, GuidIndex, ObjectStore, Reputations, SelfGuid};
use crate::target::ring::Factions;
use crate::ui_chat::{ChatEventKind, ChatLog};
use crate::ui_party::GroupState;

/// What the three watchers share to classify, name and range-gate a line.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct WatchCtx<'w> {
    pub self_guid: Res<'w, SelfGuid>,
    pub group: Res<'w, GroupState>,
    pub index: Res<'w, GuidIndex>,
    pub factions: Option<Res<'w, Factions>>,
    pub reputations: Res<'w, Reputations>,
    pub spells: Option<Res<'w, crate::ui_action::Spells>>,
    /// The creature records, whose VISIBLE_TO_GHOSTS bit `CanAttack` reads.
    pub names: Option<Res<'w, crate::names::NameCache>>,
    /// The live display ranges: the reference's `0x8629e0` table plus `CombatDeathLogRange`.
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
            self.names.as_deref(),
        )
    }

    /// A spell's display name, or `None` where the aura formatters write no line
    /// ([`super::logged_spell`]).
    fn spell_name(&self, spell_id: u32) -> Option<String> {
        super::logged_spell(self.spells.as_deref(), spell_id).map(|d| d.name.clone())
    }

    /// [`Self::spell_name`] for the aura formatter `0x62b480`, which also writes nothing for a
    /// passive spell (`Attributes & 0x40`, `0x62b4fb`).
    fn aura_spell_name(&self, spell_id: u32) -> Option<String> {
        super::logged_spell(self.spells.as_deref(), spell_id)
            .filter(|d| d.attributes & ATTR_PASSIVE == 0)
            .map(|d| d.name.clone())
    }

    /// The one-sided range test 15 reference formatters run in place of the two-ended `0x626630`.
    /// The death line passes `CombatDeathLogRange`, which `0x62c160` reads at `0x62c19c`; the aura
    /// lines pass their class's range.
    fn in_range(&self, guid: u64, range: f32, poses: &Query<&Transform>) -> bool {
        in_range(guid, range, &self.self_guid, &self.index, poses)
    }
}

/// `SPELL_ATTR_PASSIVE`, `Attributes` bit 6 (vmangos `SpellDefines.h:798`).
const ATTR_PASSIVE: u32 = 0x40;

/// Queue one single-endpoint line, gated the way its formatter gates, with the word it raises once
/// written.
fn queue_one(
    log: &ChatLog,
    ctx: &WatchCtx,
    poses: &Query<&Transform>,
    kind: ChatEventKind,
    family: Family,
    subject: (u64, UnitClass),
    range: f32,
    fills: Fills,
    word: Option<LineWord>,
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
        named: Named::Ready,
        tries: 0,
        word,
    });
}

/// The creating spell's (`UNIT_CREATED_BY_SPELL`) `Spell.dbc` `Effect[0]` values that word a death
/// as "destroyed", `0x62c320`'s byte table: totems, summoned objects, guardians.
const DESTROYED_EFFECTS: [u32; 10] = [50, 74, 87, 88, 89, 90, 104, 105, 106, 107];

/// Bit 3 of a unit's `+0xc58`: an XP award named it (`0x60a0e6`, from `0x637f1d`). Only the unit's
/// constructor clears the word (`0x5fae7a`), so the mark lives as long as the unit does,
/// released and brought back included. The death line stands down for a marked unit
/// (`0x62c232`-`0x62c23e`): the XP line ("X dies, you gain N experience.") is its sentence.
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct XpAwarded;

/// The unit-death line (`0x625190` → `0x62c160`), off the `UNIT_FIELD_HEALTH` edge as the
/// reference's watcher `0x6046f0` takes it (alive to dead at `0x6047a3`); a unit that streams in
/// dead prints nothing, and neither does one an XP award named ([`XpAwarded`]).
pub(crate) fn death_lines(
    ctx: WatchCtx,
    stores: Query<(Entity, &ObjectStore)>,
    awarded: Query<(), With<XpAwarded>>,
    mut edges: MessageReader<FieldChanged>,
    poses: Query<&Transform>,
    log: Res<ChatLog>,
) {
    for e in edges.read() {
        if !(e.unit_field(benilla_protocol::field::FIELD_UNIT_HEALTH) && e.old > 0 && e.new == 0) {
            continue;
        }
        if awarded.contains(e.entity) {
            continue;
        }
        let Ok((_, store)) = stores.get(e.entity) else {
            continue;
        };
        let guid = e.guid;
        let class = ctx.classify(guid, &stores);
        // A summoned thing is destroyed, a living one dies, by its creating spell's first effect.
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
            &log,
            &ctx,
            &poses,
            death_kind(class),
            family,
            (guid, class),
            ctx.ranges.death(),
            Fills::default(),
            None,
        );
    }
}

/// The aura lines (`0x62b480`, `0x62b800`), one per field edge:
///
/// - a `UNIT_FIELD_AURA` slot gaining a spell words `AURAADDED*` at the periodic chat types
///   (`0x604d00`'s add arm);
/// - a slot losing one words `AURAREMOVED*` at chat types `0x41`-`0x43`; a replace words both,
///   the arrival first;
/// - a `UNIT_FIELD_AURAAPPLICATIONS` byte rising on a slot whose spell did not move words
///   `AURAAPPLICATIONADDED*` (`0x604ea0`). The reference gates it on `StackAmount > 1`, which
///   holds for any count that rises; a first application moves the spell, so it is skipped.
///
/// Harmful is the slot index, `0x20`-`0x2f` (`0x61238e`), not `UNIT_FIELD_AURAFLAGS`. A line on
/// you raises `AURA_START` or `AURA_START_HARMFUL` for an arrival and `AURA_END` or
/// `AURA_END_HARMFUL` for a loss, with the spell's name (`0x62b74f`-`0x62b7ac`); a stack line
/// raises none. An arrival or a loss writes nothing for a passive spell, and a loss nothing on a
/// unit with no health (`0x62b503`-`0x62b515`), so the auras death strips leave silently.
pub(crate) fn aura_lines(
    ctx: WatchCtx,
    stores: Query<(Entity, &ObjectStore)>,
    mut edges: MessageReader<FieldChanged>,
    poses: Query<&Transform>,
    log: Res<ChatLog>,
) {
    use benilla_protocol::field::{FIELD_UNIT_AURA, FIELD_UNIT_AURAAPPLICATIONS};
    /// Slots below it are helpful (`0x61238e`).
    const FIRST_HARMFUL_SLOT: u16 = 0x20;
    const SLOTS: u16 = benilla_protocol::messages::UNIT_AURA_SLOTS as u16;
    let harmful = |slot: u16| slot >= FIRST_HARMFUL_SLOT;

    // The whole batch first: the applications leg skips slots whose `AURA` dword moved in the
    // same block.
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
        let word = |message_type: &'static str, spell: &str| {
            (class == UnitClass::Me).then(|| LineWord::with(message_type, spell))
        };
        // Arrival first, so a replace words the new aura before the old one's removal.
        if e.new != 0 {
            if let (Some(spell), Some(kind)) = (
                ctx.aura_spell_name(e.new),
                periodic_kind(class, !harmful(slot)),
            ) {
                let (family, message_type) = if harmful(slot) {
                    (super::AURAADDED_HARMFUL, "AURA_START_HARMFUL")
                } else {
                    (super::AURAADDED_HELPFUL, "AURA_START")
                };
                let word = word(message_type, &spell);
                queue_one(
                    &log,
                    &ctx,
                    &poses,
                    kind,
                    family,
                    (e.guid, class),
                    ctx.ranges.class(class),
                    Fills {
                        spell,
                        ..Default::default()
                    },
                    word,
                );
            }
        }
        // The health is read signed (`0x62b50e`, `jg`).
        let alive = stores
            .get(e.entity)
            .is_ok_and(|(_, st)| st.0.unit_health().unwrap_or(0) as i32 > 0);
        if e.old != 0 && alive {
            if let Some(spell) = ctx.aura_spell_name(e.old) {
                let message_type = if harmful(slot) {
                    "AURA_END_HARMFUL"
                } else {
                    "AURA_END"
                };
                let word = word(message_type, &spell);
                queue_one(
                    &log,
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
                    word,
                );
            }
        }
    }

    // Stack counts: four slots per dword, one byte each, holding `stack - 1`.
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
            // A rise, to two or more, on a slot whose spell stayed put.
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
                &log,
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
                None,
            );
        }
    }
}

/// The pet-loyalty line (`0x5ff860` → `0x62d440`) on `UNIT_FIELD_BYTES_1` byte 1 moving, for your
/// own pet only, at chat type `0x19` `COMBAT_MISC_INFO`. The reference prints the text through a
/// bare `"%s"`, which a family with no slots matches.
pub(crate) fn pet_loyalty_lines(
    ctx: WatchCtx,
    stores: Query<(Entity, &ObjectStore)>,
    mut edges: MessageReader<FieldChanged>,
    log: Res<ChatLog>,
) {
    for e in edges.read() {
        if !e.unit_field(benilla_protocol::field::FIELD_UNIT_BYTES_1) {
            continue;
        }
        // Byte 1, as `ObjectFields::unit_loyalty_level` reads it.
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
            word: None,
        });
    }
}

#[cfg(test)]
mod tests;
