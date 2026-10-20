//! What each pushed spell view read, and which changes of the player's state move it. The
//! reference builds a spell tooltip at the hover and keeps nothing; the feed pushes views ahead of
//! it, so it must know, per view, the inputs it was built from. A view records them as it is built
//! ([`Deps`]), the feed diffs the player-side inputs frame to frame ([`Seen`], the modifier
//! tables, the watched reagents) into what moved ([`Changes`]), and only the views that read what
//! moved are built again.

use std::collections::BTreeMap;

use benilla_formats::RangeUnit;
use benilla_protocol::ObjectFields;

use crate::items::Items;
use crate::net::{ObjectStore, Objects};
use crate::spell::usable::{slot_item_cached, SlotItem, EQUIPMENT_MASK, EQUIPMENT_SLOTS};
use crate::spell::{ModsDiff, SkillSnapshot, SpellReads, UnitField};

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

/// The player's unit fields the cost, cast and cooldown cells read ([`UnitField`]), as the values
/// they read them at (an absent field reads 0, the cast speed `None`).
#[derive(Clone, Copy, Default, PartialEq)]
struct UnitFields {
    base_mana: u32,
    base_health: u32,
    ranged_time: u32,
    /// As bits, so a NaN compares equal to itself.
    cast_speed: Option<u32>,
    /// `POWER_COST_MODIFIER` and `POWER_COST_MULTIPLIER` (as bits), by school.
    cost_modifier: [i32; 7],
    cost_multiplier: [u32; 7],
}

impl UnitFields {
    fn of(player: Option<&ObjectStore>) -> Self {
        let Some(p) = player.map(|s| &s.0) else {
            return Self::default();
        };
        Self {
            base_mana: p.unit_base_mana().unwrap_or(0),
            base_health: p.unit_base_health().unwrap_or(0),
            ranged_time: p.unit_ranged_attack_time().unwrap_or(0),
            cast_speed: p.unit_mod_cast_speed().map(f32::to_bits),
            cost_modifier: std::array::from_fn(|s| {
                p.unit_power_cost_modifier(s as u32).unwrap_or(0)
            }),
            cost_multiplier: std::array::from_fn(|s| {
                p.unit_power_cost_multiplier(s as u32)
                    .map_or(0, f32::to_bits)
            }),
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
        moved(UnitField::BaseMana, self.base_mana, prev.base_mana)
            | moved(UnitField::BaseHealth, self.base_health, prev.base_health)
            | moved(UnitField::RangedTime, self.ranged_time, prev.ranged_time)
            | if self.cast_speed != prev.cast_speed {
                UnitField::CastSpeed.bit()
            } else {
                0
            }
            | if self.cost_modifier != prev.cost_modifier {
                UnitField::PowerCostModifier.bit()
            } else {
                0
            }
            | if self.cost_multiplier != prev.cost_multiplier {
                UnitField::PowerCostMultiplier.bit()
            } else {
                0
            }
    }
}

/// What one built view read of the player-side inputs the feed watches; the pet's own inputs
/// ([`super::spell_feed::PetInputs`]) rebuild every pet view and are not recorded. Written where
/// the read is made: in the builder, and inside the cost, cast-time and equipped-item computations
/// it shares with the cast path, which report through [`SpellReads`]. The range cell's record is
/// the one predicate, `benilla_formats::min_max_range_reads_units`, kept beside the
/// `min_max_range` it describes and held to it by the reach, auto-attack target and run sweeps
/// (`deps_tests`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Deps {
    /// A `$z` expanded: the bind point.
    pub(super) home: bool,
    /// A `$g`/`$G` expanded: the branch read the active player's gender byte (`0x508214`).
    pub(super) gender: bool,
    /// The required-form test read the shapeshift form.
    pub(super) form: bool,
    /// The range cell's melee arm read the caster's and the auto-attack target's combat reach and
    /// motion; the ranged arm, given no target, reads neither.
    pub(super) range_units: bool,
    /// The chance line read these percentages, a [`Chance`] bit each.
    pub(super) avoidance: u8,
    /// The equipped-item search read these equipment slots, a bit each.
    pub(super) worn: u32,
    /// The equipped-item search asked whether the disarm flag hides a hand.
    pub(super) disarm: bool,
    /// The reputation line read the player's standing with its faction (`0x4d6370`), the wire
    /// slot on the base its race and class pick.
    pub(super) reputation: bool,
    /// The player's unit fields the cost, cast and cooldown cells read, a [`UnitField`] bit each.
    /// A pet view's cost and cast cells read the pet's own fields, which are pet inputs, so of
    /// the unit fields only the ranged attack time (read off the player) is recorded for one.
    pub(super) unit: u16,
    /// The modifier cells read: the `SpellFamilyFlags` bits of the view's spell and of each spell
    /// its text cross-references, for the ones the caster's class family owns.
    pub(super) mod_bits: u64,
    /// The skill lines read, through the view's own spell and each one its text cross-references.
    pub(super) skill_lines: Vec<u32>,
    /// The item entries the tools and reagents lines list, whose carried counts and names the
    /// lines read.
    reagents: Vec<u32>,
}

impl Deps {
    pub(super) fn chance(&mut self, which: Chance) {
        self.avoidance |= which.bit();
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

    /// The tool and reagent entries the view lists.
    pub(super) fn reagents(&self) -> &[u32] {
        &self.reagents
    }

    /// Whether `changes` moved an input this view read.
    pub(super) fn hits(&self, changes: &Changes) -> bool {
        changes.everything
            || (changes.home && self.home)
            || (changes.gender && self.gender)
            || (changes.form && self.form)
            || (changes.range_units && self.range_units)
            || changes.avoidance & self.avoidance != 0
            || changes.worn & self.worn != 0
            || (changes.disarm && self.disarm)
            || (changes.reputation && self.reputation)
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

impl SpellReads for Deps {
    fn unit(&mut self, field: UnitField) {
        self.unit |= field.bit();
    }

    fn item_search(&mut self, slots: u32) {
        self.worn |= slots;
        self.disarm |= slots != 0;
    }

    fn skill_line(&mut self, line: u32) {
        Deps::skill_line(self, line);
    }
}

/// What moved since the frame before, in the terms of [`Deps`].
#[derive(Debug, Default)]
pub(super) struct Changes {
    /// An input no view can be ruled out of: every view is due.
    pub(super) everything: bool,
    pub(super) home: bool,
    pub(super) gender: bool,
    pub(super) form: bool,
    pub(super) range_units: bool,
    /// A [`Chance`] bit each.
    pub(super) avoidance: u8,
    /// The equipment slots whose item moved in a way the search can tell, a bit each.
    pub(super) worn: u32,
    pub(super) disarm: bool,
    /// A reputation standing moved.
    pub(super) reputation: bool,
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
            && !self.gender
            && !self.form
            && !self.range_units
            && self.avoidance == 0
            && self.worn == 0
            && !self.disarm
            && !self.reputation
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

/// What the range cell's melee arm reads of two units: the caster's combat reach and its
/// auto-attack target's, as bit patterns (`None` with none engaged, where the arm reads the
/// caster's reach twice), and whether the moving bonus applies, the one fact of the units' motion
/// the cell's number follows: both move at run speed
/// ([`benilla_formats::UnitMotion::moves_at_run_speed`]) with a target engaged to read.
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) struct RangeSeen {
    caster_reach: u32,
    attack_target_reach: Option<u32>,
    bonus: bool,
}

impl RangeSeen {
    pub(super) fn of(caster: &RangeUnit, attack_target: Option<&RangeUnit>) -> Self {
        Self {
            caster_reach: caster.reach.to_bits(),
            attack_target_reach: attack_target.map(|u| u.reach.to_bits()),
            bonus: caster.motion.moves_at_run_speed()
                && attack_target.is_some_and(|u| u.motion.moves_at_run_speed()),
        }
    }
}

/// The player-side inputs one frame saw, which the pushed views were built against.
#[derive(Clone, PartialEq)]
pub(super) struct Seen {
    /// The bind point `$z` names.
    home: Option<String>,
    /// The gender byte `$g`/`$G` branch on (`0x508214`), 0 without a player as the views read it.
    gender: u8,
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
    /// The caster's and its auto-attack target's reach, and whether the moving bonus applies.
    range_units: RangeSeen,
    /// The unit fields the cost, cast and cooldown cells read.
    unit: UnitFields,
    /// The wire standings, slot by slot, and the race and class whose base they ride on, which
    /// the reputation line's standing reads.
    reputations: Vec<i32>,
    race_class: (u8, u8),
}

impl Seen {
    pub(super) fn of(
        player: Option<&ObjectStore>,
        home: Option<&str>,
        range_units: RangeSeen,
        objects: &Objects,
        items: &Items,
    ) -> Self {
        Seen {
            home: home.map(str::to_string),
            gender: player.and_then(|s| s.0.unit_gender()).unwrap_or(0),
            form: player.map_or(0, |s| s.0.unit_shapeshift_form()),
            skills: crate::spell::skill_snapshot(player),
            worn: player
                .map(|s| std::array::from_fn(|i| slot_item_cached(s, i as u8, objects, items))),
            disarmed: player.is_some_and(crate::items::is_disarmed),
            avoidance: Chance::ALL
                .map(|which| player.and_then(|s| which.read(&s.0)).map(f32::to_bits)),
            range_units,
            unit: UnitFields::of(player),
            reputations: Vec::new(),
            race_class: player.map_or((0, 0), |s| {
                (s.0.unit_race().unwrap_or(0), s.0.unit_class().unwrap_or(0))
            }),
        }
    }

    /// The standings the frame saw, which the reputation line reads.
    pub(super) fn with_reputations(mut self, reputations: &crate::net::Reputations) -> Self {
        self.reputations = reputations.slots.iter().map(|&(_, w)| w).collect();
        self
    }

    /// What differs from the frame `prev` saw, the modifiers and reagents apart. A player
    /// appearing or going moves every view: one built without a player read none of its state.
    pub(super) fn changes_since(&self, prev: &Seen) -> Changes {
        let mut changes = Changes {
            everything: self.worn.is_some() != prev.worn.is_some(),
            home: self.home != prev.home,
            gender: self.gender != prev.gender,
            form: self.form != prev.form,
            range_units: self.range_units != prev.range_units,
            disarm: self.disarmed != prev.disarmed,
            reputation: self.reputations != prev.reputations || self.race_class != prev.race_class,
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
