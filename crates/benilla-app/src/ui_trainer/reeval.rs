//! **The trainer's state re-evaluator** — the reference's `0x4d7d40`, transcribed.
//!
//! The service `state` byte a trainer window shows is a wire value only on the **first** frame.
//! The real client keeps twelve triggers on the open window — money, level, a skill change, a
//! spell learned or removed, a talent, the pet's spellbook, and the two async spell-data callbacks
//! — and each of them runs `0x4d7d40`, which **discards** every service's wire state and
//! re-derives it from the player's own spellbook, skills, level and pet, then recounts the groups
//! and fires `TRAINER_UPDATE`. It is *not* called by the builder or the
//! packet handler, so a freshly opened window shows the server's bytes verbatim until the first
//! trigger.
//!
//! Until 2333 benilla treated the wire state as durable and, on a purchase, asked the server for a
//! fresh list — a round trip the reference never makes (it registers no handler for
//! `SMSG_TRAINER_BUY_SUCCEEDED` at all) — so a level gained or a skill point earned with the window
//! open left the rows stale. This module is the re-derivation; [`super::feed_trainer`] runs it on
//! the same triggers and fires the same event.
//!
//! **What it deliberately reproduces:** the skill leg's not-found exit jumps *past* the state
//! write (`0x4d8082` → `0x4d8123`), so a service whose required skill line the player does not
//! hold at all comes out **green** — the opposite of the server's red. That is the 1.12 client's
//! own behaviour, pinned by `a_skill_line_the_player_lacks_entirely_reads_green`, and the
//! reason a fresh Blacksmithing recipe list can flip from red to green under a level-up without
//! the player touching anything.
//!
//! **The whole law**:
//! the admission gate, the two `trainerType == 1` writes of state 3, the pet-spell legs as early
//! exits, the required-ability leg's pet variant, and the twelve triggers named to their
//! handlers. What is not built is the one thing no vmangos server reaches: a type-1 (mount)
//! trainer's layout, where a row re-derived to state 2 also has its group key severed
//! (`0x4d82c1`) — noted, not transcribed, until that layout exists.

use std::collections::BTreeSet;

use benilla_formats::{LearnEffect, SkillLineCatalog, SpellCatalog};
use benilla_protocol::messages::trainer_spell_state::{GRAY, GREEN, RED};
use benilla_protocol::messages::TrainerSpell;

/// The `trainerType` the reference gates its state-3 writes on (`ds:0xb73a08 == 1`).
const TRAINER_TYPE_STATE_THREE: u32 = 1;
/// The state the type-1 required-ability leg writes instead of RED (`0x4d83a4`): a row no filter
/// can ever show (`SetTrainerServiceTypeFilter` sets bits 0/1/2 only, `0x4da390`).
const HIDDEN: u8 = 3;

/// One `PLAYER_SKILL_INFO` slot as the re-evaluator reads it: the line id, the step word
/// (`+0x84a`) and `value + permBonus` (`+0x84c` + `+0x852`) — the bonus added only when the
/// value is non-zero, the same guard the skill-pair reader `0x5ea460` has (verified, 2336).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SkillSlot {
    pub skill_id: u32,
    pub step: u32,
    pub value_plus_perm: i32,
}

/// The player's pet, when the bar names one whose object has streamed: its level and its own
/// spellbook (the `IsSpellKnown` pet path scans the pet's book, not the player's).
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
    /// `UNIT_FIELD_LEVEL`.
    pub level: u32,
    /// The occupied `PLAYER_SKILL_INFO` slots.
    pub skills: Vec<SkillSlot>,
    /// The pet, or `None` — "no pet" is itself a verdict for a pet-spell service.
    pub pet: Option<PetView>,
}

impl PlayerView {
    fn skill(&self, skill_id: u32) -> Option<&SkillSlot> {
        self.skills.iter().find(|s| s.skill_id == skill_id)
    }
}

/// `IsSpellKnown(unit, id) || KnownHigherRank(unit, id)` — `0x60c740` OR `0x60c8d0`, against
/// whichever unit's book `known` is.
fn known_or_higher(id: u32, known: &BTreeSet<u32>, skill_lines: &SkillLineCatalog) -> bool {
    known.contains(&id) || skill_lines.higher_rank_known(id, known)
}

/// Re-derive one service's state byte from the player, as `0x4d7d40` does per row. The wire
/// state is an input only through the admission gate: a row whose wire spell has no `Spell.dbc`
/// record is skipped whole and keeps it (`0x4d7dcd` → next row); every other row restarts at 0
/// (`0x4d7dec`).
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
    // 2 · per learn effect of the WIRE spell, in slot order; every write overwrites the last.
    let mut learn_effects = 0u32;
    let mut known_effects = 0u32;
    // The row's LEARN_SPELL target (`[ebp-0x28]`) and the pet its LEARN_PET_SPELL resolved
    // (`[ebp-0x18]`) — both read again by the required-ability leg.
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
                    // `0x4d7e3e`: at a type-1 trainer a rank the player has already passed is
                    // hidden outright — counted as a learn effect, never as a known one, so
                    // step 3 cannot turn it into 2.
                    state = HIDDEN;
                } else if known_or_higher(t, &player.known, skill_lines) {
                    known_effects += 1;
                }
            }
            LearnEffect::SkillStep { skill, step } => {
                // The player's step word for that line against the effect's value (`0x4d7f17`);
                // no slot for the line ⇒ no write (`0x4d7ecf`).
                if player.skill(skill).is_some_and(|s| s.step >= step) {
                    state = GRAY;
                }
            }
            LearnEffect::PetSpell(t) => {
                learn_effects += 1;
                // No pet ⇒ unavailable (`0x4d803c`), and the loop ends there.
                let Some(pet) = &player.pet else {
                    state = RED;
                    break;
                };
                pet_resolved = Some(pet);
                if known_or_higher(t, &pet.known, skill_lines) {
                    // Known by the PET (`IsSpellKnown`'s pet path scans the pet's book).
                    known_effects += 1;
                } else if wire.req_level > 1 && pet.level < u32::from(wire.req_level) {
                    // A pet below the row's level ⇒ unavailable (`0x4d7fac`), loop ends.
                    state = RED;
                    break;
                }
            }
        }
    }
    // 3 · every learn effect known ⇒ known (`0x4d7fc6`), over whatever the loop left.
    if learn_effects > 0 && known_effects == learn_effects {
        state = GRAY;
    }
    // 4 · only while still 0: the skill leg. A slot for the line, below the requirement ⇒
    //     unavailable (`0x4d811f`); NO slot ⇒ no write at all (`0x4d8082` jumps past it) — the
    //     reference's own green-for-a-skill-you-lack.
    if state == GREEN && wire.req_skill != 0 {
        if let Some(slot) = player.skill(wire.req_skill) {
            if slot.value_plus_perm < wire.req_skill_value as i32 {
                state = RED;
            }
        }
    }
    // 5 · still 0: the required abilities, against the PET's book when the row resolved one
    //     (`0x4d81c4`) and the player's otherwise; the first missing one decides. On the pet the
    //     verdict is always unavailable (`0x4d8204`). On the player at a type-1 trainer it is the
    //     hidden state (`0x4d83a4`) when the missing ability is exactly the rank before what the
    //     row teaches — `SkillLineAbility.forward(missing) == taught` — and unavailable
    //     (`0x4d83ad`) for any other missing ability.
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
    //     Still 0: a level requirement above 1 the player has not reached (`0x4d8239`); no
    //     trainer-type variant.
    if state == GREEN && wire.req_level > 1 && player.level < u32::from(wire.req_level) {
        state = RED;
    }
    state
}

/// Re-derive every listed service in place — the whole of `0x4d7d40`'s per-row pass. The group
/// recount the reference does next (`0x4d8260`–`0x4d83c9`) is the engine's, synthesized from the
/// pushed states (`benilla_ui::script::trainer`), so nothing is counted here.
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
