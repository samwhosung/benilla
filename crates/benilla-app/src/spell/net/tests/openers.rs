//! The opener list `0xb700b0` that the lock resolver `0x5f83d0` scans from index 0: the known
//! spells whose `Effect[0]` is `OPEN_LOCK` (`0x4b265e`), in the order the server taught them.
//! Lock 43, the Hyacinth Mushroom's, has one slot that both 6478 "Opening" and 22810 "Opening -
//! No Text" clear, so the order alone picks the spell the click casts.

use super::*;
use crate::target::lock::{resolve_lock, GoFacts, LockOutcome};
use benilla_formats::{LockSlot, OpenLock, SpellDisplay, GO_STATE_READY, LOCK_KEY_SKILL};

const OPENING: u32 = 6478;
const OPENING_NO_TEXT: u32 = 22810;
/// A second rank of "Opening", made up, between the two ids.
const OPENING_2: u32 = 6479;
/// A spell with no lock effect.
const FIREBALL: u32 = 133;

/// `Effect[0]` `OPEN_LOCK` (0x21) on LockType 13.
fn opener() -> SpellDisplay {
    SpellDisplay {
        effects: [0x21, 0, 0],
        open_lock: Some(OpenLock {
            lock_type: 13,
            effect: 0,
        }),
        ..Default::default()
    }
}

fn spells() -> Spells {
    let mut spells = Spells::empty_for_tests();
    spells.catalog = benilla_formats::SpellCatalog::from_displays(
        [
            (OPENING, opener()),
            (OPENING_NO_TEXT, opener()),
            (OPENING_2, opener()),
            (FIREBALL, SpellDisplay::default()),
        ]
        .into_iter()
        .collect(),
    );
    spells
}

/// `SMSG_INITIAL_SPELLS` with `ids` in wire order.
fn book(ids: &[u32], spells: &Spells) -> PlayerActions {
    let mut actions = PlayerActions::default();
    spell_book(
        ids.to_vec(),
        vec![],
        &mut actions,
        &mut Cooldowns::default(),
        (
            Some(spells),
            &crate::items::Items::default(),
            &crate::spell::SpellModifiers::default(),
            crate::spell::CategoryPad::for_list(false, None),
        ),
    );
    actions
}

/// What the resolver casts for lock 43: its one slot, LockType 13 at skill 0, on a closed object.
fn opens(actions: &PlayerActions, spells: &Spells) -> LockOutcome {
    let slot = [LockSlot {
        key_type: LOCK_KEY_SKILL,
        index: 13,
        skill: 0,
        action: 0,
    }];
    let mut objects = crate::ui_items::TestObjects::new();
    resolve_lock(
        &slot,
        &actions.openers,
        Some(spells),
        None,
        None,
        &objects.get(),
        GoFacts {
            state: GO_STATE_READY,
            flag_locked: false,
            level: 0,
        },
        &mut None,
    )
}

/// The replay learns the list in wire order (`0x5dea8b`-`0x5deab9`), each opener appended
/// (`0x4b2675`-`0x4b2689`), and the resolver takes the first that clears the slot (`0x5f8511`).
#[test]
fn the_first_opener_the_server_listed_opens_the_lock() {
    let spells = spells();
    let actions = book(&[OPENING_NO_TEXT, FIREBALL, OPENING], &spells);
    assert_eq!(
        opens(&actions, &spells),
        LockOutcome::OpenBySpell(OPENING_NO_TEXT)
    );
    let actions = book(&[OPENING, OPENING_NO_TEXT], &spells);
    assert_eq!(opens(&actions, &spells), LockOutcome::OpenBySpell(OPENING));
}

/// `SMSG_LEARNED_SPELL` appends at the tail (`0x5e620d` → `0x5e9c20` → `0x4b25b0`).
#[test]
fn a_learned_opener_goes_to_the_tail() {
    let spells = spells();
    let mut actions = book(&[FIREBALL], &spells);
    let errors = UiErrorKeys::default();
    let mut tab = LearnedInTab::default();
    learned_spell(
        OPENING_NO_TEXT,
        &mut actions,
        Some(&spells),
        &errors,
        &mut tab,
    );
    learned_spell(OPENING, &mut actions, Some(&spells), &errors, &mut tab);
    assert_eq!(
        opens(&actions, &spells),
        LockOutcome::OpenBySpell(OPENING_NO_TEXT)
    );
}

/// `SMSG_SUPERCEDED_SPELL` removes the old rank (`0x4b2f5b` → `0x4b2c50`) and appends the new
/// one (`0x4b2f68` → `0x4b25b0`), so the new rank goes behind every other opener.
#[test]
fn a_superseded_opener_moves_its_new_rank_to_the_tail() {
    let spells = spells();
    let mut actions = book(&[OPENING, OPENING_NO_TEXT], &spells);
    superceded_spell(
        OPENING,
        OPENING_2,
        &mut actions,
        Some(&spells),
        &UiErrorKeys::default(),
        &mut LearnedInTab::default(),
    );
    assert_eq!(
        opens(&actions, &spells),
        LockOutcome::OpenBySpell(OPENING_NO_TEXT)
    );
}

/// `SMSG_REMOVED_SPELL` takes the spell out (`0x5e43e3` → `0x5e9fe0` → `0x4b2c50`), closing the
/// gap without reordering the rest (`0x4b2d74`-`0x4b2d8e`).
#[test]
fn an_unlearned_opener_leaves_the_rest_in_order() {
    let spells = spells();
    let mut actions = book(&[OPENING_2, OPENING_NO_TEXT, OPENING], &spells);
    removed_spell(
        OPENING_2,
        &mut actions,
        Some(&spells),
        &UiErrorKeys::default(),
    );
    assert_eq!(
        opens(&actions, &spells),
        LockOutcome::OpenBySpell(OPENING_NO_TEXT)
    );
}

/// The registrar appends with no duplicate check (`0x4b2665`-`0x4b2689`) and the unlearn takes
/// the last copy (`0x4b2d60`-`0x4b2d67`), so an opener learned twice keeps its first place.
#[test]
fn a_relearned_opener_is_listed_twice_and_an_unlearn_takes_the_last_copy() {
    let spells = spells();
    let mut actions = book(&[OPENING_NO_TEXT, OPENING], &spells);
    let errors = UiErrorKeys::default();
    learned_spell(
        OPENING_NO_TEXT,
        &mut actions,
        Some(&spells),
        &errors,
        &mut LearnedInTab::default(),
    );
    assert_eq!(actions.openers, [OPENING_NO_TEXT, OPENING, OPENING_NO_TEXT]);
    removed_spell(OPENING_NO_TEXT, &mut actions, Some(&spells), &errors);
    assert_eq!(
        opens(&actions, &spells),
        LockOutcome::OpenBySpell(OPENING_NO_TEXT)
    );
}

/// The supersede unlists the old rank twice: in `0x4b2f50` (`0x4b2f5b`), then in the player's
/// unlearn of it (`0x5e6392` → `0x5ea24c`), so an old rank listed twice leaves no copy.
#[test]
fn a_supersede_unlists_the_old_rank_twice() {
    let spells = spells();
    let mut actions = book(&[OPENING], &spells);
    let errors = UiErrorKeys::default();
    let mut tab = LearnedInTab::default();
    learned_spell(OPENING, &mut actions, Some(&spells), &errors, &mut tab);
    superceded_spell(
        OPENING,
        OPENING_2,
        &mut actions,
        Some(&spells),
        &errors,
        &mut tab,
    );
    assert_eq!(actions.openers, [OPENING_2]);
    assert_eq!(
        opens(&actions, &spells),
        LockOutcome::OpenBySpell(OPENING_2)
    );
}
