//! What each pushed spell view read, and which changes of the player's state move it. The
//! reference builds a spell tooltip at the hover and keeps nothing; the feed pushes views ahead of
//! it, so it must know, per view, the inputs it was built from. A view records them as it is built
//! ([`Deps`]), the feed diffs the player-side inputs frame to frame ([`Seen`], the modifier
//! tables, the watched reagents) into what moved ([`Changes`]), and only the views that read what
//! moved are built again.

use std::collections::BTreeMap;

use benilla_protocol::ObjectFields;

use crate::net::ObjectStore;
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
    /// The equipped-item search read these worn slots, a bit each.
    pub(super) worn: u32,
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
    /// The worn slots whose guid moved, a bit each.
    pub(super) worn: u32,
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
    /// The 19 worn-slot guids the required-item line's colour follows (`0x5f0c50`); `None`
    /// without a player.
    worn: Option<[u64; 19]>,
    /// The [`Chance`] percentages as bit patterns.
    avoidance: [Option<u32>; 4],
    /// The caster's and its auto-attack target's combat reach as bit patterns.
    reach: (Option<u32>, Option<u32>),
}

impl Seen {
    pub(super) fn of(
        player: Option<&ObjectStore>,
        home: Option<&str>,
        attack_target_reach: Option<f32>,
    ) -> Self {
        Seen {
            home: home.map(str::to_string),
            form: player.map_or(0, |s| s.0.unit_shapeshift_form()),
            skills: crate::spell::skill_snapshot(player),
            worn: player
                .map(|s| std::array::from_fn(|i| s.0.player_inv_slot(i as u8).unwrap_or(0))),
            avoidance: Chance::ALL
                .map(|which| player.and_then(|s| which.read(&s.0)).map(f32::to_bits)),
            reach: (
                player.map(|s| s.0.unit_combat_reach().to_bits()),
                attack_target_reach.map(f32::to_bits),
            ),
        }
    }

    /// What differs from the frame `prev` saw, the modifiers and reagents apart.
    pub(super) fn changes_since(&self, prev: &Seen) -> Changes {
        let mut changes = Changes {
            home: self.home != prev.home,
            form: self.form != prev.form,
            reach: self.reach != prev.reach,
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
            // A player appearing or going: every slot's guid is new.
            _ => crate::spell::usable::WORN_SLOTS,
        };
        if self.skills != prev.skills {
            changes.skill_lines = crate::spell::changed_skill_lines(&prev.skills, &self.skills);
        }
        changes
    }
}
