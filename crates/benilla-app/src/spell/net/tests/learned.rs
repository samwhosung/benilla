//! The known spells in learn order ([`PlayerActions::learned`]): the order the profession lines'
//! and the craft types' lists keep (`0x5e9d72`, `0x5e9e0f`), which the lists' builds walk.

use super::*;

const A: u32 = 300;
const B: u32 = 100;
const C: u32 = 200;
const D: u32 = 150;

fn spells() -> Spells {
    let mut spells = Spells::empty_for_tests();
    spells.catalog = benilla_formats::SpellCatalog::from_displays(
        [A, B, C, D]
            .into_iter()
            .map(|id| (id, benilla_formats::SpellDisplay::default()))
            .collect(),
    );
    spells
}

/// The login list keeps its wire order (`0x5dea8b`-`0x5deab9`); a learn appends (`0x5e9d72`); an
/// unlearn closes the gap (`0x5ea123`); a supersede unlearns the old rank and learns the new one
/// at the tail (`0x5e6392`, `0x5e63a4`).
#[test]
fn the_known_spells_keep_their_learn_order() {
    let spells = spells();
    let mut actions = PlayerActions::default();
    spell_book(
        vec![A, B, C],
        vec![],
        &mut actions,
        &mut Cooldowns::default(),
        (
            Some(&spells),
            &crate::items::Items::default(),
            &crate::spell::SpellModifiers::default(),
            crate::spell::CategoryPad::for_list(false, None),
        ),
    );
    assert_eq!(actions.learned, [A, B, C]);
    let errors = UiErrorKeys::default();
    let mut tab = LearnedInTab::default();
    learned_spell(D, &mut actions, Some(&spells), &errors, &mut tab);
    assert_eq!(actions.learned, [A, B, C, D]);
    removed_spell(B, &mut actions, Some(&spells), &errors);
    assert_eq!(actions.learned, [A, C, D]);
    superceded_spell(A, B, &mut actions, Some(&spells), &errors, &mut tab);
    assert_eq!(actions.learned, [C, D, B]);
}
