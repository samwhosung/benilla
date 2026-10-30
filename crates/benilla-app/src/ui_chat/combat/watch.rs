//! The combat-log lines that come off descriptor edges, not packets, as in the reference: the
//! death line (the unit-death reflex `0x625190`; 1.12 has no death opcode), the aura lines (the
//! `UNIT_FIELD_AURA` and `UNIT_FIELD_AURAAPPLICATIONS` callbacks `0x604d00` and `0x604ea0`,
//! through `0x612320`, `0x6123f0` and `0x612450`) and the pet-loyalty line (`0x5ff860`). Each is
//! written inside the update packet's notify pass (`0x465277` → `0x465330`), so the packet's end
//! words them ([`crate::field_callbacks`]), over the world its packet left.

use bevy::prelude::*;

use super::{
    aura_gone_kind, classify, death_kind, in_range, periodic_kind, CombatLogRanges, Family, Fills,
    LineWord, Named, PendingCombat, UnitClass, Variant,
};
use crate::net::{AuraFieldsChanged, FieldChanged, GuidIndex, ObjectStore, Reputations, SelfGuid};
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

    /// A spell's display name for the aura formatter `0x62b480`, or `None` where it writes no
    /// line: [`super::logged_spell`]'s gates and a passive spell (`Attributes & 0x40`, `0x62b4fb`).
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
/// written, at the place of the field edge `at` that words it ([`push_at`]).
fn queue_one(
    at: crate::lanes::Stamp,
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
    push_at(
        at,
        log,
        PendingCombat {
            kind,
            family,
            variant: Variant::of(subject.1, UnitClass::Creature),
            subject: subject.0,
            object: 0,
            fills,
            named: Named::Ready,
            tries: 0,
            word,
        },
    );
}

/// Queue `line` at the place of the field edge `at` that words it: the reference's field callback
/// writes it inside the update's handler (`0x465330`), so it fires there among what the packets
/// queued, not where this system runs.
fn push_at(at: crate::lanes::Stamp, log: &ChatLog, line: PendingCombat) {
    crate::lanes::firing(at, || log.push_combat(line));
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

/// The unit-death line (`0x625190` → `0x62c160`), off the `UNIT_FIELD_HEALTH` edge `e` as the
/// reference's watcher `0x6046f0` takes it (alive to dead at `0x6047a3`), written where
/// `PresentUnitDeath` comes in the death's legs (`0x6058ae`, `crate::target`'s `died`); a unit
/// that streams in dead prints nothing, and neither does one an XP award named ([`XpAwarded`]).
pub(crate) fn death_line(world: &mut World, e: FieldChanged) {
    if let Err(err) = world.run_system_cached_with(word_death, e) {
        // A world with no chat log words nothing.
        debug!("combat log: no death line: {err}");
    }
}

/// [`death_line`] over the world as the packet left it.
fn word_death(
    In(e): In<FieldChanged>,
    ctx: WatchCtx,
    stores: Query<(Entity, &ObjectStore)>,
    awarded: Query<(), With<XpAwarded>>,
    poses: Query<&Transform>,
    log: Res<ChatLog>,
) {
    if awarded.contains(e.entity) {
        return;
    }
    let Ok((_, store)) = stores.get(e.entity) else {
        return;
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
        crate::lanes::stamp(),
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

/// The packet's aura, stack and loyalty lines, off its field `edges` and the aura watcher's
/// `calls`, each at its watcher's place.
pub(crate) fn lines(world: &mut World, edges: &[FieldChanged], calls: &[AuraFieldsChanged]) {
    use benilla_protocol::field::{FIELD_UNIT_AURAAPPLICATIONS, FIELD_UNIT_BYTES_1};
    const SLOTS: u16 = benilla_protocol::messages::UNIT_AURA_SLOTS as u16;
    let edges: Vec<FieldChanged> = edges
        .iter()
        .filter(|e| {
            e.unit_field(FIELD_UNIT_BYTES_1)
                || e.unit_array_slot(FIELD_UNIT_AURAAPPLICATIONS, SLOTS / 4)
                    .is_some()
        })
        .copied()
        .collect();
    if edges.is_empty() && calls.is_empty() {
        return;
    }
    if let Err(err) = world.run_system_cached_with(word_lines, (edges, calls.to_vec())) {
        debug!("combat log: no aura lines: {err}");
    }
}

/// [`lines`] over the world as the packet left it.
fn word_lines(
    In((edges, calls)): In<(Vec<FieldChanged>, Vec<AuraFieldsChanged>)>,
    ctx: WatchCtx,
    stores: Query<(Entity, &ObjectStore)>,
    poses: Query<&Transform>,
    log: Res<ChatLog>,
    mut notices: Option<ResMut<crate::field_watch::NoticeLane>>,
) {
    aura_lines(
        &ctx,
        &stores,
        &poses,
        &log,
        notices.as_deref_mut(),
        &calls,
        &edges,
    );
    pet_loyalty_lines(&ctx, &stores, &log, &edges);
}

/// The aura lines (`0x62b480`, `0x62b800`):
///
/// - each call of the aura watcher `0x604d00` walks the 48 slots twice: first every slot whose
///   old aura is gone or replaced words `AURAREMOVED*` at chat types `0x41`-`0x43` (`0x604d40`-
///   `0x604dd8`, `0x612320`), then every slot with a new aura words `AURAADDED*` at the periodic
///   chat types (`0x604df5`-`0x604e85`, `0x6123f0`). A slot holds an aura when its spell is
///   nonzero and its `UNIT_FIELD_AURAFLAGS` nibble has a bit of 0xe (`0x604d79`);
/// - a `UNIT_FIELD_AURAAPPLICATIONS` byte rising words `AURAAPPLICATIONADDED*` (`0x604ea0`).
///
/// Harmful is the slot index, `0x20`-`0x2f` (`0x61238e`), not `UNIT_FIELD_AURAFLAGS`. A line on
/// you raises `AURA_START` or `AURA_START_HARMFUL` for an arrival and `AURA_END` or
/// `AURA_END_HARMFUL` for a loss, with the spell's name (`0x62b74f`-`0x62b7ac`); a stack line
/// raises none. An arrival or a loss writes nothing for a passive spell, and a loss nothing on a
/// unit with no health (`0x62b503`-`0x62b515`), so the auras death strips leave silently.
fn aura_lines(
    ctx: &WatchCtx,
    stores: &Query<(Entity, &ObjectStore)>,
    poses: &Query<&Transform>,
    log: &ChatLog,
    mut notices: Option<&mut crate::field_watch::NoticeLane>,
    calls: &[AuraFieldsChanged],
    edges: &[FieldChanged],
) {
    use benilla_protocol::field::FIELD_UNIT_AURAAPPLICATIONS;
    const SLOTS: u16 = benilla_protocol::messages::UNIT_AURA_SLOTS as u16;

    for call in calls {
        aura_call(ctx, stores, poses, log, notices.as_deref_mut(), call);
    }

    // Stack counts: four slots per dword, one byte each, holding `stack - 1`. Each byte is its
    // own watch (`0x604ea0`), and `0x612450` compares the bytes alone (`cmp`, `jbe`).
    for e in edges.iter().filter(|e| {
        e.unit_array_slot(FIELD_UNIT_AURAAPPLICATIONS, SLOTS / 4)
            .is_some()
    }) {
        let word = e.index - FIELD_UNIT_AURAAPPLICATIONS;
        let Ok((_, store)) = stores.get(e.entity) else {
            continue; // gone in the same drain
        };
        for byte in 0..4u16 {
            let slot = word * 4 + byte;
            let (before, now) = ((e.old >> (byte * 8)) as u8, (e.new >> (byte * 8)) as u8);
            if now <= before {
                continue;
            }
            // `0x62b800` reads the slot's spell dword as the descriptor holds it then, every
            // block of the packet written (`0x62b822`), and writes only for a spell whose
            // `StackAmount` is above 1 (`0x62b87c`, `jle`).
            let spell_id = store.0.unit_aura_ids().nth(usize::from(slot)).unwrap_or(0);
            let Some(display) = super::logged_spell(ctx.spells.as_deref(), spell_id)
                .filter(|d| d.stack_amount as i32 > 1)
            else {
                continue;
            };
            let class = ctx.classify(e.guid, stores);
            let Some(kind) = periodic_kind(class, !harmful(slot)) else {
                continue;
            };
            queue_one(
                e.at,
                log,
                ctx,
                poses,
                kind,
                if harmful(slot) {
                    super::AURAAPPLICATIONADDED_HARMFUL
                } else {
                    super::AURAAPPLICATIONADDED_HELPFUL
                },
                (e.guid, class),
                ctx.ranges.class(class),
                Fills {
                    spell: display.name.clone(),
                    // The byte plus one, as a byte (`0x62b9dc`).
                    amount: i64::from(now.wrapping_add(1)),
                    ..Default::default()
                },
                None,
            );
        }
    }
}

/// Whether an aura slot is harmful: slots below 0x20 are helpful (`0x61238e`).
fn harmful(slot: u16) -> bool {
    slot >= 0x20
}

/// One slot's aura in the watcher's dwords: its spell when the spell is nonzero and the slot's
/// `AURAFLAGS` nibble has a bit of 0xe (`0x604d5d`-`0x604d79`), else `None`.
fn held(span: &[u32; crate::net::AURA_WATCH_SPAN], slot: usize) -> Option<u32> {
    const SLOTS: usize = benilla_protocol::messages::UNIT_AURA_SLOTS as usize;
    let nibble = (span[SLOTS + slot / 8] >> ((slot % 8) * 4)) & 0xe;
    (span[slot] != 0 && nibble != 0).then_some(span[slot])
}

/// One call of the aura watcher `0x604d00`: the removals, then the adds, each slot's legs in its
/// handler's order. On the active player (`0x5fa6d0`) each also notifies the bar's slots that stand
/// for its spell in the player's book, with no send (`0x4e5de0`): a removal's ahead of its line
/// (`0x61237e`, before `0x5ff290` and `0x62b480`), an add's after it, the spell read off the
/// slot's field (`0x612433`-`0x612440`), so a toggle's slot repaints its active icon
/// (`0x4e55f0`).
fn aura_call(
    ctx: &WatchCtx,
    stores: &Query<(Entity, &ObjectStore)>,
    poses: &Query<&Transform>,
    log: &ChatLog,
    mut notices: Option<&mut crate::field_watch::NoticeLane>,
    call: &AuraFieldsChanged,
) {
    const SLOTS: usize = benilla_protocol::messages::UNIT_AURA_SLOTS as usize;
    let mine = ctx.self_guid.0 == Some(call.guid);
    let mut notify = |spell: u32| {
        if let Some(lane) = notices.as_deref_mut().filter(|_| mine) {
            crate::lanes::firing(call.at, || {
                lane.write(crate::field_watch::FieldNotice::SpellSlots { spell })
            });
        }
    };
    let class = ctx.classify(call.guid, stores);
    let word = |message_type: &'static str, spell: &str| {
        (class == UnitClass::Me).then(|| LineWord::with(message_type, spell))
    };
    // The health is read signed (`0x62b50e`, `jg`).
    let alive = call.health as i32 > 0;
    // A slot is removed when its old aura is not still there, the same spell under a nibble with
    // a bit of 0xe (`0x604da3`-`0x604dc2`).
    for slot in 0..SLOTS {
        let Some(old) = held(&call.old, slot) else {
            continue;
        };
        if held(&call.new, slot) == Some(old) {
            continue;
        }
        notify(old);
        if !alive {
            continue;
        }
        let Some(spell) = ctx.aura_spell_name(old) else {
            continue;
        };
        let message_type = if harmful(slot as u16) {
            "AURA_END_HARMFUL"
        } else {
            "AURA_END"
        };
        let word = word(message_type, &spell);
        queue_one(
            call.at,
            log,
            ctx,
            poses,
            aura_gone_kind(class),
            super::AURAREMOVED,
            (call.guid, class),
            ctx.ranges.class(class),
            Fills {
                spell,
                ..Default::default()
            },
            word,
        );
    }
    // A slot is added when its new aura was not already there, the same spell under an old
    // nibble with a bit of 0xe (`0x604e31`-`0x604e6c`).
    for slot in 0..SLOTS {
        let Some(new) = held(&call.new, slot) else {
            continue;
        };
        if held(&call.old, slot) == Some(new) {
            continue;
        }
        let helpful = !harmful(slot as u16);
        let (Some(spell), Some(kind)) = (ctx.aura_spell_name(new), periodic_kind(class, helpful))
        else {
            notify(new);
            continue;
        };
        let (family, message_type) = if helpful {
            (super::AURAADDED_HELPFUL, "AURA_START")
        } else {
            (super::AURAADDED_HARMFUL, "AURA_START_HARMFUL")
        };
        let word = word(message_type, &spell);
        queue_one(
            call.at,
            log,
            ctx,
            poses,
            kind,
            family,
            (call.guid, class),
            ctx.ranges.class(class),
            Fills {
                spell,
                ..Default::default()
            },
            word,
        );
        notify(new);
    }
}

/// The pet-loyalty line (`0x5ff860` → `0x62d440`) on `UNIT_FIELD_BYTES_1` byte 1 moving, for your
/// own pet only, at chat type `0x19` `COMBAT_MISC_INFO`. The reference prints the text through a
/// bare `"%s"`, which a family with no slots matches.
fn pet_loyalty_lines(
    ctx: &WatchCtx,
    stores: &Query<(Entity, &ObjectStore)>,
    log: &ChatLog,
    edges: &[FieldChanged],
) {
    for e in edges {
        if !e.unit_field(benilla_protocol::field::FIELD_UNIT_BYTES_1) {
            continue;
        }
        // Byte 1, as `ObjectFields::unit_loyalty_level` reads it.
        let (before, level) = (((e.old >> 8) & 0xff) as u8, ((e.new >> 8) & 0xff) as u8);
        if level == before || level == 0 || before == 0 {
            continue;
        }
        if ctx.classify(e.guid, stores) != UnitClass::MyPet {
            continue;
        }
        push_at(
            e.at,
            log,
            PendingCombat {
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
            },
        );
    }
}

#[cfg(test)]
mod tests;
