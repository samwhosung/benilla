//! The orders the profession lines' and the craft types' lists keep
//! ([`PlayerActions::profession_order`], [`PlayerActions::craft_order`]), which their builds walk.

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

/// The profession lists' order takes every learn, a known spell's again included, as
/// `LearnSpell 0x5e9c20` checks for none (`0x5e9d72`; the replay `0x5deaac`, `0x5e61c0`,
/// `0x5e63a4`), and an unlearn takes out the last copy (`0x5ea0fe`); the craft lists' order adds
/// a spell it lacks (`0x5e9db3`) and an unlearn takes out the first (`0x5ea1b8`).
#[test]
fn the_list_orders_keep_their_learn_order() {
    let spells = spells();
    let mut actions = PlayerActions::default();
    spell_book(
        vec![A, B, A],
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
    assert_eq!(actions.profession_order, [A, B, A]);
    assert_eq!(actions.craft_order, [A, B]);
    let errors = UiErrorKeys::default();
    let mut tab = LearnedInTab::default();
    learned_spell(B, &mut actions, Some(&spells), &errors, &mut tab);
    assert_eq!(actions.profession_order, [A, B, A, B]);
    assert_eq!(actions.craft_order, [A, B]);
    removed_spell(A, &mut actions, Some(&spells), &errors);
    assert_eq!(actions.profession_order, [A, B, B]);
    assert_eq!(actions.craft_order, [B]);
    superceded_spell(B, A, &mut actions, Some(&spells), &errors, &mut tab);
    assert_eq!(actions.profession_order, [A, B, A]);
    assert_eq!(actions.craft_order, [A]);
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
    assert_eq!(actions.profession_order, [A, B, C]);
    let errors = UiErrorKeys::default();
    let mut tab = LearnedInTab::default();
    learned_spell(D, &mut actions, Some(&spells), &errors, &mut tab);
    assert_eq!(actions.profession_order, [A, B, C, D]);
    removed_spell(B, &mut actions, Some(&spells), &errors);
    assert_eq!(actions.profession_order, [A, C, D]);
    superceded_spell(A, B, &mut actions, Some(&spells), &errors, &mut tab);
    assert_eq!(actions.profession_order, [C, D, B]);
}
