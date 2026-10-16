//! The item tooltip's lock check, the reference's `0x5da5e0`: whether the active player can open
//! an item's lock, and by what. It colours the `LOCKED` line (`0x52be0c`..`0x52be3d`).
//!
//! The engine reads no DBCs, so the app pushes the `Lock.dbc` rows ([`LockSlotView`]) once and the
//! player's means of opening ([`LockOpeners`]) on change; the walk is here.

use std::collections::HashSet;

use super::UiScript;

/// One `Lock.dbc` slot as `0x5da5e0` reads it: the key type at `lock+0x4`, the index at `+0x24` and
/// the skill at `+0x44`, each 8 wide. Its action column (`+0x64`) is not read: unlike the game
/// object walk (`0x5f83d0`), an item's lock has no state to gate a slot on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LockSlotView {
    /// 0 empty, 1 a key item, 2 a skill.
    pub key_type: u32,
    /// The key's item entry (type 1), or the `LockType.dbc` index an opening spell names (type 2).
    pub index: u32,
    /// The skill a type-2 slot needs; 0 takes the item's level times 5 (`0x5da6ae`).
    pub skill: u32,
}

/// A known spell with an `SPELL_EFFECT_OPEN_LOCK` effect (33), in the order of the known-spell
/// list the walk scans (`0xb700b8`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LockOpener {
    /// The effect's `EffectMiscValue`, matched against a skill slot's index (`0x5da69f`).
    pub lock_type: u32,
    /// The effect's integer value for the player (`0x6e3760`), compared to the slot's skill.
    pub points: i32,
}

/// What the active player can open locks with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LockOpeners {
    /// The known open-lock spells, in scan order.
    pub spells: Vec<LockOpener>,
    /// The entries of the items the player carries, equipment, bags and keyring (`0x622270` at
    /// mode 0, as the walk asks it at `0x5da734`).
    pub held: HashSet<u32>,
}

/// The walk's answer: how the colour of `LOCKED` is picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockCheck {
    /// A key or skill slot with no opener (`0x5da5e0` returns nonzero): red.
    Closed,
    /// A known spell reaches the slot's skill: the margin ramp `0x529fa0` of its `points` against
    /// the `required` skill.
    Skill { required: i32, points: i32 },
    /// The player holds the key, or the lock has no key or skill slot, or there is no active
    /// player (`0x5da60b`..`0x5da61a` returns 0): the green `0xc0d420`.
    Open,
}

/// `0x5da5e0(lock, level)`. `level` is the item record's `+0x38`, the fallback behind a slot whose
/// skill is 0. `openers` is `None` with no active player, which opens everything.
///
/// Walks the slots in order: a skill slot (type 2) scans the known spells for an open-lock effect
/// whose misc value is its index and whose points reach its requirement (`0x5da6f8 jge`); a key
/// slot (type 1) looks for the item. The first to succeed ends the walk. A slot of any other type
/// is skipped. A type 1 or 2 slot makes the lock closed unless one succeeds (`0x5da64e`,
/// `0x5da725`).
pub fn check_lock(slots: &[LockSlotView], level: u32, openers: Option<&LockOpeners>) -> LockCheck {
    let Some(openers) = openers else {
        return LockCheck::Open;
    };
    let mut closed = false;
    for slot in slots {
        match slot.key_type {
            2 => {
                closed = true;
                let required = if slot.skill != 0 {
                    slot.skill as i32
                } else {
                    level.wrapping_mul(5) as i32
                };
                for spell in &openers.spells {
                    if spell.lock_type == slot.index && spell.points >= required {
                        return LockCheck::Skill {
                            required,
                            points: spell.points,
                        };
                    }
                }
            }
            1 => {
                closed = true;
                if openers.held.contains(&slot.index) {
                    return LockCheck::Open;
                }
            }
            _ => {}
        }
    }
    if closed {
        LockCheck::Closed
    } else {
        LockCheck::Open
    }
}

impl UiScript {
    /// Pushes `Lock.dbc` whole, by lock id, once at load: a clicked tooltip cannot repaint on a late
    /// answer. The reference reads its loaded store (`0xc0dae4`).
    pub fn set_lock_rows(&mut self, rows: std::collections::HashMap<u32, Vec<LockSlotView>>) {
        self.model_mut().lock_rows = rows;
    }

    /// Pushes what the active player opens locks with, on change. Never pushed, there is no active
    /// player and every lock reads as open.
    pub fn set_lock_openers(&mut self, openers: LockOpeners) {
        self.model_mut().lock_openers = Some(openers);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(index: u32, skill: u32) -> LockSlotView {
        LockSlotView {
            key_type: 2,
            index,
            skill,
        }
    }

    fn key(entry: u32) -> LockSlotView {
        LockSlotView {
            key_type: 1,
            index: entry,
            skill: 0,
        }
    }

    fn picker(points: i32) -> LockOpeners {
        LockOpeners {
            spells: vec![LockOpener {
                lock_type: 1,
                points,
            }],
            held: HashSet::new(),
        }
    }

    #[test]
    fn a_skill_slot_opens_at_its_requirement_and_not_below() {
        let lock = [skill(1, 25)];
        assert_eq!(check_lock(&lock, 0, Some(&picker(24))), LockCheck::Closed);
        assert_eq!(
            check_lock(&lock, 0, Some(&picker(25))),
            LockCheck::Skill {
                required: 25,
                points: 25
            }
        );
    }

    #[test]
    fn a_skill_slot_wants_the_locktype_it_names() {
        // Lock type 2 (herbalism) against a lockpicking spell (type 1).
        assert_eq!(
            check_lock(&[skill(2, 1)], 0, Some(&picker(300))),
            LockCheck::Closed
        );
    }

    #[test]
    fn a_zero_skill_takes_five_times_the_item_level() {
        let lock = [skill(1, 0)];
        assert_eq!(check_lock(&lock, 20, Some(&picker(99))), LockCheck::Closed);
        assert_eq!(
            check_lock(&lock, 20, Some(&picker(100))),
            LockCheck::Skill {
                required: 100,
                points: 100
            }
        );
    }

    #[test]
    fn a_key_slot_opens_by_the_held_entry() {
        let held = |entries: &[u32]| LockOpeners {
            spells: Vec::new(),
            held: entries.iter().copied().collect(),
        };
        assert_eq!(
            check_lock(&[key(5060)], 0, Some(&held(&[5060, 1]))),
            LockCheck::Open
        );
        assert_eq!(
            check_lock(&[key(5060)], 0, Some(&held(&[1]))),
            LockCheck::Closed
        );
    }

    #[test]
    fn a_lock_without_key_or_skill_slots_is_open() {
        let none = LockOpeners::default();
        assert_eq!(check_lock(&[], 0, Some(&none)), LockCheck::Open);
        let other = LockSlotView {
            key_type: 3,
            index: 9,
            skill: 9,
        };
        assert_eq!(
            check_lock(&[LockSlotView::default(), other], 0, Some(&none)),
            LockCheck::Open
        );
    }

    #[test]
    fn the_first_slot_that_opens_ends_the_walk() {
        let mut both = picker(30);
        both.held.insert(7);
        // A key not held, then a skill that reaches: the skill.
        let lock = [key(8), skill(1, 25)];
        assert_eq!(
            check_lock(&lock, 0, Some(&both)),
            LockCheck::Skill {
                required: 25,
                points: 30
            }
        );
        // The skill first, with the key held after it: the skill still wins; when it falls short
        // the key opens it, and the lock is not closed for the skill that failed.
        let lock = [skill(1, 25), key(7)];
        assert_eq!(
            check_lock(&lock, 0, Some(&both)),
            LockCheck::Skill {
                required: 25,
                points: 30
            }
        );
        let mut weak = picker(10);
        weak.held.insert(7);
        assert_eq!(check_lock(&lock, 0, Some(&weak)), LockCheck::Open);
    }

    #[test]
    fn with_no_active_player_the_lock_is_open() {
        assert_eq!(
            check_lock(&[skill(1, 300), key(1)], 0, None),
            LockCheck::Open
        );
    }
}
