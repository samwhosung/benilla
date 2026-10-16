//! The inputs of the item tooltip's lock check (`0x5da5e0`): `Lock.dbc` and `LockType.dbc`'s names
//! pushed whole once, and what the active player opens locks with, pushed on change. The walk and
//! the colour are the engine's ([`benilla_ui::script::check_lock`]).

use std::collections::{BTreeSet, HashMap, HashSet};

use bevy::prelude::*;

use benilla_formats::{LockCatalog, SpellCatalog};
use benilla_ui::script::{LockOpener, LockOpeners, LockSlotView, UiScript};

use crate::ui_script::gate;

/// Pushes every `Lock.dbc` row and `LockType.dbc` name once per VM. Both load together; without
/// `LockType.dbc` every lock kind reads `UNKNOWN`, as a missing row does.
pub(super) fn feed_lock_rows(
    script: Option<NonSendMut<UiScript>>,
    locks: Option<Res<crate::go_templates::Locks>>,
    lock_types: Option<Res<crate::go_templates::LockTypes>>,
    mut pushed: Local<crate::ui_script::VmMemo<bool>>,
) {
    let Some(mut script) = script else {
        return;
    };
    if *pushed.get(&script) {
        return;
    }
    let Some(locks) = locks else {
        return;
    };
    let rows = row_views(&locks.0);
    script.set_lock_rows(rows);
    script.set_lock_type_names(
        lock_types
            .iter()
            .flat_map(|t| t.0.names())
            .map(|(id, name)| (id, name.to_string()))
            .collect(),
    );
    *pushed.get(&script) = true;
}

/// Every row as the reference holds it, all eight slots in order: the walk skips a type 0 slot,
/// and the line under `LOCKED` reads the first slot's type whatever it is (`0x52be5d`).
fn row_views(catalog: &LockCatalog) -> HashMap<u32, Vec<LockSlotView>> {
    catalog
        .rows()
        .map(|(id, slots)| {
            let slots = slots
                .iter()
                .map(|s| LockSlotView {
                    key_type: s.key_type,
                    index: s.index,
                    skill: s.skill,
                })
                .collect();
            (id, slots)
        })
        .collect()
}

/// What the player opens a lock with: each known spell with an open-lock effect, in the scan order
/// of the known-spell list, with the skill it provides now (`0x6e3760`), and the item entries
/// carried (`0x622270`, mode 0). `skill` is the player's skill in a spell's own line.
fn lock_openers(
    known: &BTreeSet<u32>,
    spells: Option<&SpellCatalog>,
    skill: &dyn Fn(u32) -> u32,
    held: HashSet<u32>,
) -> LockOpeners {
    let opening = known.iter().filter_map(|&id| {
        let spell = spells?.get(id)?;
        Some(LockOpener {
            lock_type: spell.open_lock_type()?,
            points: spell.open_lock_skill(skill(id))?,
        })
    });
    LockOpeners {
        spells: opening.collect(),
        held,
    }
}

/// Pushes [`LockOpeners`] when it changes. No player streamed pushes nothing, which the walk reads
/// as every lock open, as the reference's null player does.
pub(super) fn feed_lock_openers(
    script: Option<NonSendMut<UiScript>>,
    mut inv: crate::items::Inventory,
    actions: Res<crate::ui_action::PlayerActions>,
    spells: Option<Res<crate::ui_action::Spells>>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    mut last: Local<crate::ui_script::VmMemo<Option<LockOpeners>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (last, vm_reset) = last.get_reset(&script);
    // The state is a function of these inputs alone: the carried items (the object index and the
    // descriptor slots that point at them), the book, the spell catalog, and the skills the
    // opening spells' values scale by (the descriptor).
    let objects_moved = inv.changes.moved();
    let self_changed = !inv.self_changed.is_empty();
    let actions_changed = actions.is_changed();
    let spells_changed = spells.as_ref().is_some_and(|r| r.is_changed());
    gate::trace(
        "feed_lock_openers",
        &[
            ("vm_reset", vm_reset),
            ("objects", objects_moved),
            ("self", self_changed),
            ("actions", actions_changed),
            ("spells", spells_changed),
        ],
    );
    let gate = gate::Gate::new(
        vm_reset || objects_moved || self_changed || actions_changed || spells_changed,
    );
    if gate.skip() {
        return;
    }
    let Some(store) = inv.self_store.iter().next() else {
        return;
    };
    let skill_catalog = skill_lines.as_deref().map(|s| &s.catalog);
    let held = crate::ui_items::stack_counts(&store.0, &inv.objects)
        .into_keys()
        .collect();
    let state = lock_openers(
        &actions.spells,
        spells.as_deref().map(|s| &s.catalog),
        &|id| crate::spell::spell_skill_value(Some(store), skill_catalog, id),
        held,
    );
    if last.as_ref() != Some(&state) {
        gate.audit("feed_lock_openers", "the lock openers");
        *last = Some(state.clone());
        script.set_lock_openers(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::{LockSlot, OpenLock, SpellDisplay, MAX_LOCK_SLOTS};

    /// Pick Lock: lock type 1, whose value tracks the player's lockpicking by 5 per 5 skill.
    fn pick_lock() -> SpellDisplay {
        SpellDisplay {
            open_lock: Some(OpenLock {
                effect: 0,
                lock_type: 1,
            }),
            effect_real_points_per_level: [5.0, 0.0, 0.0],
            ..SpellDisplay::default()
        }
    }

    #[test]
    fn the_openers_are_the_known_open_lock_spells_with_their_current_points() {
        let catalog = SpellCatalog::from_displays(HashMap::from([
            (1804, pick_lock()),
            (133, SpellDisplay::default()),
            (6477, pick_lock()),
        ]));
        let known = BTreeSet::from([133, 1804, 6477, 9999]);
        let at =
            |skill: u32| lock_openers(&known, Some(&catalog), &|_| skill, HashSet::from([5060]));
        let openers = at(150);
        assert_eq!(
            openers.spells,
            [
                LockOpener {
                    lock_type: 1,
                    points: 150
                },
                LockOpener {
                    lock_type: 1,
                    points: 150
                }
            ],
            "ascending spell id, Fireball and the unknown id left out"
        );
        assert_eq!(openers.held, HashSet::from([5060]));
        // The value follows the skill, so a skill-up changes the state and re-pushes it.
        assert_ne!(at(155), openers);
        assert!(lock_openers(&known, None, &|_| 150, HashSet::new())
            .spells
            .is_empty());
    }

    #[test]
    fn a_row_keeps_all_its_slots_in_place() {
        let mut slots = [LockSlot::default(); MAX_LOCK_SLOTS];
        slots[1] = LockSlot {
            key_type: 1,
            index: 5060,
            skill: 0,
            action: 0,
        };
        slots[3] = LockSlot {
            key_type: 2,
            index: 1,
            skill: 25,
            action: 4,
        };
        let rows = row_views(&LockCatalog::from_rows([
            (7, slots),
            (8, [LockSlot::default(); MAX_LOCK_SLOTS]),
        ]));
        assert_eq!(rows[&7].len(), MAX_LOCK_SLOTS);
        assert_eq!(rows[&7][0], LockSlotView::default(), "slot 0 stays empty");
        assert_eq!(
            rows[&7][1],
            LockSlotView {
                key_type: 1,
                index: 5060,
                skill: 0
            }
        );
        assert_eq!(
            rows[&7][3],
            LockSlotView {
                key_type: 2,
                index: 1,
                skill: 25
            }
        );
        assert_eq!(
            rows[&8].len(),
            MAX_LOCK_SLOTS,
            "a row of empty slots stays a row"
        );
    }
}
