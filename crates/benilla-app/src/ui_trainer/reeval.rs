//! The trainer's state re-evaluator, the reference's `0x4d7d40`: on any of its twelve triggers it
//! re-derives every service's state from the player's book, skills, level and pet, so the wire
//! state lasts only until the first. A skill line the player lacks entirely reads green where the
//! server sends red (`0x4d8082` → `0x4d8123`), as in the 1.12 client. Not built: at a type-1
//! trainer the reference also severs the group key of a row re-derived to state 2 (`0x4d82c1`).

use std::collections::BTreeSet;

use benilla_formats::{LearnEffect, SkillLineCatalog, SpellCatalog};
use benilla_protocol::messages::trainer_spell_state::{GRAY, GREEN, RED};
use benilla_protocol::messages::TrainerSpell;

/// The `trainerType` the reference gates its state-3 writes on (`ds:0xb73a08 == 1`).
const TRAINER_TYPE_STATE_THREE: u32 = 1;
/// The state the type-1 required-ability leg writes instead of RED (`0x4d83a4`): a row no filter
/// can ever show (`SetTrainerServiceTypeFilter` sets bits 0/1/2 only, `0x4da390`).
const HIDDEN: u8 = 3;

/// One `PLAYER_SKILL_INFO` slot: the permanent bonus adds only to a non-zero value, as the
/// skill-pair reader `0x5ea460` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SkillSlot {
    pub skill_id: u32,
    pub step: u32,
    pub value_plus_perm: i32,
}

/// The player's streamed pet, with its own book: `IsSpellKnown`'s pet path scans the pet's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct PetView {
    pub level: u32,
    pub known: BTreeSet<u32>,
}

/// Everything `0x4d7d40` reads off the player, gathered once per re-evaluation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct PlayerView {
    /// The player's known-spell set (`[0xb710fc]`'s bitmap).
    pub known: BTreeSet<u32>,
    pub level: u32,
    /// The occupied `PLAYER_SKILL_INFO` slots.
    pub skills: Vec<SkillSlot>,
    /// `None` is itself a verdict for a pet-spell service.
    pub pet: Option<PetView>,
}

impl PlayerView {
    fn skill(&self, skill_id: u32) -> Option<&SkillSlot> {
        self.skills.iter().find(|s| s.skill_id == skill_id)
    }
}

/// `IsSpellKnown(unit, id) || KnownHigherRank(unit, id)` (`0x60c740`, `0x60c8d0`), against
/// whichever unit's book `known` is.
fn known_or_higher(id: u32, known: &BTreeSet<u32>, skill_lines: &SkillLineCatalog) -> bool {
    known.contains(&id) || skill_lines.higher_rank_known(id, known)
}

/// One row of `0x4d7d40`: a wire spell with no `Spell.dbc` record keeps its wire state
/// (`0x4d7dcd`); every other restarts at 0 (`0x4d7dec`).
pub(super) fn re_derive(
    wire: &TrainerSpell,
    trainer_type: u32,
    spells: &SpellCatalog,
    skill_lines: &SkillLineCatalog,
    player: &PlayerView,
) -> u8 {
    // 0 · the admission gate.
    if spells.get(wire.spell).is_none() {
        return wire.state;
    }
    // 1 · the wire state is discarded; the state restarts at 0.
    let mut state = GREEN;
    // 2 · per learn effect of the wire spell, in slot order; every write overwrites the last.
    let mut learn_effects = 0u32;
    let mut known_effects = 0u32;
    // Both read again by the required-ability leg.
    let mut taught: Option<u32> = None;
    let mut pet_resolved: Option<&PetView> = None;
    for effect in spells.learn_effects(wire.spell) {
        match *effect {
            LearnEffect::Spell(t) => {
                learn_effects += 1;
                taught = Some(t);
                if trainer_type == TRAINER_TYPE_STATE_THREE
                    && skill_lines.higher_rank_known(t, &player.known)
                {
                    // `0x4d7e3e`: at a type-1 trainer a rank the player has passed is hidden,
                    // counted as a learn effect but never a known one, so step 3 cannot gray it.
                    state = HIDDEN;
                } else if known_or_higher(t, &player.known, skill_lines) {
                    known_effects += 1;
                }
            }
            LearnEffect::SkillStep { skill, step } => {
                // Against the player's step word (`0x4d7f17`); no slot, no write (`0x4d7ecf`).
                if player.skill(skill).is_some_and(|s| s.step >= step) {
                    state = GRAY;
                }
            }
            LearnEffect::PetSpell(t) => {
                learn_effects += 1;
                // No pet: unavailable, and the loop ends (`0x4d803c`).
                let Some(pet) = &player.pet else {
                    state = RED;
                    break;
                };
                pet_resolved = Some(pet);
                if known_or_higher(t, &pet.known, skill_lines) {
                    // Known by the pet: `IsSpellKnown`'s pet path scans the pet's book.
                    known_effects += 1;
                } else if wire.req_level > 1 && pet.level < u32::from(wire.req_level) {
                    // A pet below the row's level: unavailable, and the loop ends (`0x4d7fac`).
                    state = RED;
                    break;
                }
            }
        }
    }
    // 3 · every learn effect known: known (`0x4d7fc6`), over whatever the loop left.
    if learn_effects > 0 && known_effects == learn_effects {
        state = GRAY;
    }
    // 4 · still 0: the skill leg. A slot below the requirement is unavailable (`0x4d811f`); no
    //     slot writes nothing (`0x4d8082` jumps past it).
    if state == GREEN && wire.req_skill != 0 {
        if let Some(slot) = player.skill(wire.req_skill) {
            if slot.value_plus_perm < wire.req_skill_value as i32 {
                state = RED;
            }
        }
    }
    // 5 · still 0: the first missing required ability, in the pet's book when the row resolved a
    //     pet (`0x4d81c4`, always unavailable, `0x4d8204`), else the player's: hidden at type 1
    //     when it is the rank before `taught` (`0x4d83a4`), otherwise unavailable (`0x4d83ad`).
    if state == GREEN {
        let (book, on_pet) = match pet_resolved {
            Some(pet) => (&pet.known, true),
            None => (&player.known, false),
        };
        let missing = wire
            .req_spells
            .iter()
            .copied()
            .filter(|&id| id != 0)
            .find(|&id| !known_or_higher(id, book, skill_lines));
        if let Some(missing) = missing {
            let previous_rank_of_taught =
                taught.is_some_and(|t| skill_lines.rank_successor(missing) == Some(t));
            state =
                if !on_pet && trainer_type == TRAINER_TYPE_STATE_THREE && previous_rank_of_taught {
                    HIDDEN
                } else {
                    RED
                };
        }
    }
    //     Still 0: a level requirement above 1 the player lacks, at any type (`0x4d8239`).
    if state == GREEN && wire.req_level > 1 && player.level < u32::from(wire.req_level) {
        state = RED;
    }
    state
}

/// Every row in place; the reference's group recount (`0x4d8260`-`0x4d83c9`) is the engine's.
pub(super) fn re_derive_all(
    services: &mut [TrainerSpell],
    trainer_type: u32,
    spells: &SpellCatalog,
    skill_lines: &SkillLineCatalog,
    player: &PlayerView,
) {
    for wire in services.iter_mut() {
        wire.state = re_derive(wire, trainer_type, spells, skill_lines, player);
    }
}
