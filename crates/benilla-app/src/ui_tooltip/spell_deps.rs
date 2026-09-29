//! What each pushed spell view read, and which changes of the player's state move it. The
//! reference builds a spell tooltip at the hover and keeps nothing; the feed pushes views ahead of
//! it, so it must know, per view, the inputs it was built from. A view records them as it is built
//! ([`Deps`]), the feed diffs the player-side inputs frame to frame ([`Seen`], the modifier
//! tables, the watched reagents) into what moved ([`Changes`]), and only the views that read what
//! moved are built again.

use std::collections::BTreeMap;

use benilla_protocol::ObjectFields;

use crate::items::Items;
use crate::net::{ObjectStore, Objects};
use crate::spell::usable::{slot_item_cached, SlotItem, EQUIPMENT_MASK, EQUIPMENT_SLOTS};
use crate::spell::{ModsDiff, SkillSnapshot};

/// The four percentages a chance-to-X line reads (`0x52f5b1`), in the order [`Seen`] keeps them.
#[derive(Clone, Copy)]
pub(super) enum Chance {
    Block,
    Dodge,
    Parry,
    Crit,
}

impl Chance {
    pub(super) const ALL: [Chance; 4] = [Chance::Block, Chance::Dodge, Chance::Parry, Chance::Crit];

    /// The player's live percentage, already a percent on the wire.
    pub(super) fn read(self, player: &ObjectFields) -> Option<f32> {
        match self {
            Chance::Block => player.player_block_percentage(),
            Chance::Dodge => player.player_dodge_percentage(),
            Chance::Parry => player.player_parry_percentage(),
            Chance::Crit => player.player_crit_percentage(),
        }
    }

    /// The bit a [`Deps::avoidance`] or [`Changes::avoidance`] mask holds it at.
    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// One of the player's unit fields a cost, cast or cooldown cell reads, a bit of [`Deps::unit`].
#[derive(Clone, Copy)]
pub(super) enum UnitField {
    Level,
    BaseMana,
    MaxHealth,
    /// `UNIT_FIELD_MAXPOWER` of a type; one past the five reads nothing.
    MaxPower(u8),
    RangedTime,
}

impl UnitField {
    pub(super) fn bit(self) -> u16 {
        match self {
            UnitField::Level => 1,
            UnitField::BaseMana => 1 << 1,
            UnitField::MaxHealth => 1 << 2,
            UnitField::MaxPower(ty) if ty < 5 => 1 << (3 + ty),
            UnitField::MaxPower(_) => 0,
            UnitField::RangedTime => 1 << 8,
        }
    }
}

/// The player's unit fields those cells read, as the values they read them at (an absent field
/// reads 0).
#[derive(Clone, Copy, Default, PartialEq)]
struct UnitFields {
    level: u32,
    base_mana: u32,
    max_health: u32,
    max_power: [u32; 5],
    ranged_time: u32,
}

impl UnitFields {
    fn of(player: Option<&ObjectStore>) -> Self {
        let Some(p) = player.map(|s| &s.0) else {
            return Self::default();
        };
        Self {
            level: p.unit_level().unwrap_or(0),
            base_mana: p.unit_base_mana().unwrap_or(0),
            max_health: p.unit_max_health().unwrap_or(0),
            max_power: std::array::from_fn(|ty| p.unit_max_power(ty as u8).unwrap_or(0)),
            ranged_time: p.unit_ranged_attack_time().unwrap_or(0),
        }
    }

    /// The [`UnitField`] bits that differ.
    fn diff(&self, prev: &Self) -> u16 {
        let moved = |field: UnitField, now: u32, before: u32| {
            if now != before {
                field.bit()
            } else {
                0
            }
        };
        let mut bits = moved(UnitField::Level, self.level, prev.level)
            | moved(UnitField::BaseMana, self.base_mana, prev.base_mana)
            | moved(UnitField::MaxHealth, self.max_health, prev.max_health)
            | moved(UnitField::RangedTime, self.ranged_time, prev.ranged_time);
        for ty in 0..5u8 {
            bits |= moved(
                UnitField::MaxPower(ty),
                self.max_power[usize::from(ty)],
                prev.max_power[usize::from(ty)],
            );
        }
        bits
    }
}

/// What one built view read of the player-side inputs the feed watches; the pet's own inputs
/// ([`super::spell_feed::PetInputs`]) rebuild every pet view and are not recorded. Written where
/// the builder reads, so a read cannot be added without saying what it depends on.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Deps {
    /// A `$z` expanded: the bind point.
    pub(super) home: bool,
    /// The required-form test read the shapeshift form.
    pub(super) form: bool,
    /// The range cell read the caster's or the auto-attack target's combat reach.
    pub(super) reach: bool,
    /// The chance line read these percentages, a [`Chance`] bit each.
    pub(super) avoidance: u8,
    /// The equipped-item search read these equipment slots, a bit each.
    pub(super) worn: u32,
    /// The equipped-item search asked whether the disarm flag hides a hand.
    pub(super) disarm: bool,
    /// The player's unit fields the cost, cast and cooldown cells read, a [`UnitField`] bit each.
    /// A pet view's cost and cast cells read the pet's own fields, which are pet inputs, so of
    /// the unit fields only the ranged attack time (read off the player) is recorded for one.
    pub(super) unit: u16,
    /// The modifier cells read: the `SpellFamilyFlags` bits of the view's spell and of each spell
    /// its text cross-references, for the ones the caster's class family owns.
    pub(super) mod_bits: u64,
    /// The skill lines read, through the view's own spell and each one its text cross-references.
    pub(super) skill_lines: Vec<u32>,
    /// The reagent entries the reagents line lists.
    reagents: Vec<u32>,
}

impl Deps {
    pub(super) fn chance(&mut self, which: Chance) {
        self.avoidance |= which.bit();
    }

    pub(super) fn unit(&mut self, field: UnitField) {
        self.unit |= field.bit();
    }

    /// The equipped-item search ran over the equipment slots in `slots`.
    pub(super) fn item_search(&mut self, slots: u32) {
        self.worn |= slots;
        self.disarm |= slots != 0;
    }

    pub(super) fn skill_line(&mut self, line: u32) {
        if !self.skill_lines.contains(&line) {
            self.skill_lines.push(line);
        }
    }

    pub(super) fn reagent(&mut self, entry: u32) {
        if !self.reagents.contains(&entry) {
            self.reagents.push(entry);
        }
    }

    /// The reagent entries the view lists.
    pub(super) fn reagents(&self) -> &[u32] {
        &self.reagents
    }

    /// Whether `changes` moved an input this view read.
    pub(super) fn hits(&self, changes: &Changes) -> bool {
        changes.everything
            || (changes.home && self.home)
            || (changes.form && self.form)
            || (changes.reach && self.reach)
            || changes.avoidance & self.avoidance != 0
            || changes.worn & self.worn != 0
            || (changes.disarm && self.disarm)
            || changes.unit & self.unit != 0
            || changes.mod_bits & self.mod_bits != 0
            || self
                .skill_lines
                .iter()
                .any(|line| changes.skill_lines.contains(line))
            || self
                .reagents
                .iter()
                .any(|entry| changes.reagents.contains(entry))
    }
}

/// What moved since the frame before, in the terms of [`Deps`].
#[derive(Debug, Default)]
pub(super) struct Changes {
    /// An input no view can be ruled out of: every view is due.
    pub(super) everything: bool,
    pub(super) home: bool,
    pub(super) form: bool,
    pub(super) reach: bool,
    /// A [`Chance`] bit each.
    pub(super) avoidance: u8,
    /// The equipment slots whose item moved in a way the search can tell, a bit each.
    pub(super) worn: u32,
    pub(super) disarm: bool,
    /// A [`UnitField`] bit each.
    pub(super) unit: u16,
    /// The `SpellFamilyFlags` bits with a changed modifier cell.
    pub(super) mod_bits: u64,
    pub(super) skill_lines: Vec<u32>,
    pub(super) reagents: Vec<u32>,
}

impl Changes {
    pub(super) fn is_empty(&self) -> bool {
        !self.everything
            && !self.home
            && !self.form
            && !self.reach
            && self.avoidance == 0
            && self.worn == 0
            && !self.disarm
            && self.unit == 0
            && self.mod_bits == 0
            && self.skill_lines.is_empty()
            && self.reagents.is_empty()
    }

    /// The modifier tables' change. A new class family changes which spells the tables reach at
    /// all, so it is everything.
    pub(super) fn with_mods(mut self, diff: ModsDiff) -> Self {
        self.mod_bits = diff.bits;
        self.everything |= diff.class_family;
        self
    }

    /// The watched reagents whose count or resolved name moved.
    pub(super) fn with_reagents(mut self, prev: &Reagents, now: &Reagents) -> Self {
        self.reagents = now
            .iter()
            .filter(|(entry, state)| prev.get(entry) != Some(state))
            .map(|(&entry, _)| entry)
            .collect();
        self
    }
}

/// Per reagent on show, `(owned count, name resolved)`: what the reagents line renders (the
/// inline red, `0x854120`).
pub(super) type Reagents = BTreeMap<u32, (u32, bool)>;

/// The player-side inputs one frame saw, which the pushed views were built against.
#[derive(Clone, PartialEq)]
pub(super) struct Seen {
    /// The bind point `$z` names.
    home: Option<String>,
    /// The form the required-form line's colour follows (`0x52f1e3`).
    form: u8,
    /// The skills the descriptions' per-level terms scale by ([`crate::spell::skill_snapshot`]).
    skills: SkillSnapshot,
    /// What the equipped-item search learns of each of the 23 equipment slots (`0x5f0c50`); `None`
    /// without a player.
    worn: Option<[SlotItem; EQUIPMENT_SLOTS as usize]>,
    /// The disarm flag, which hides a hand from that search.
    disarmed: bool,
    /// The [`Chance`] percentages as bit patterns.
    avoidance: [Option<u32>; 4],
    /// The caster's and its auto-attack target's combat reach as bit patterns.
    reach: (Option<u32>, Option<u32>),
    /// The unit fields the cost, cast and cooldown cells read.
    unit: UnitFields,
}

impl Seen {
    pub(super) fn of(
        player: Option<&ObjectStore>,
        home: Option<&str>,
        attack_target_reach: Option<f32>,
        objects: &Objects,
        items: &Items,
    ) -> Self {
        Seen {
            home: home.map(str::to_string),
            form: player.map_or(0, |s| s.0.unit_shapeshift_form()),
            skills: crate::spell::skill_snapshot(player),
            worn: player
                .map(|s| std::array::from_fn(|i| slot_item_cached(s, i as u8, objects, items))),
            disarmed: player.is_some_and(crate::items::is_disarmed),
            avoidance: Chance::ALL
                .map(|which| player.and_then(|s| which.read(&s.0)).map(f32::to_bits)),
            reach: (
                player.map(|s| s.0.unit_combat_reach().to_bits()),
                attack_target_reach.map(f32::to_bits),
            ),
            unit: UnitFields::of(player),
        }
    }

    /// What differs from the frame `prev` saw, the modifiers and reagents apart.
    pub(super) fn changes_since(&self, prev: &Seen) -> Changes {
        let mut changes = Changes {
            home: self.home != prev.home,
            form: self.form != prev.form,
            reach: self.reach != prev.reach,
            disarm: self.disarmed != prev.disarmed,
            unit: self.unit.diff(&prev.unit),
            ..Changes::default()
        };
        for (which, (now, before)) in Chance::ALL
            .iter()
            .zip(self.avoidance.iter().zip(&prev.avoidance))
        {
            if now != before {
                changes.avoidance |= which.bit();
            }
        }
        changes.worn = match (&self.worn, &prev.worn) {
            (Some(now), Some(before)) => now
                .iter()
                .zip(before)
                .enumerate()
                .fold(0, |mask, (slot, (now, before))| {
                    mask | u32::from(now != before) << slot
                }),
            (None, None) => 0,
            // A player appearing or going: every slot's item is new.
            _ => EQUIPMENT_MASK,
        };
        if self.skills != prev.skills {
            changes.skill_lines = crate::spell::changed_skill_lines(&prev.skills, &self.skills);
        }
        changes
    }
}
