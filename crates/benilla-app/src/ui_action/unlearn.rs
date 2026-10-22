//! The unlearn's bar clear: `UnlearnSpell 0x5e9fe0` ends in `0x4e5e20` (`0x5ea253`), which clears
//! every slot that stands for the spell.

use bevy::prelude::*;

use super::PlayerActions;

/// Drains the unlearned spells the removed-spell message queued ([`PlayerActions::unlearned`]).
pub(super) fn clear_unlearned_slots(mut actions: ResMut<PlayerActions>) {
    if actions.unlearned.is_empty() {
        return;
    }
    actions.unlearned.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::Items;
    use crate::net::{ClientCommand, NetCommands};
    use crate::ui_macro::{Bound, BoundSpell, MacroBoundSpells};
    use benilla_protocol::messages::{
        ActionButton, ItemUseSpell, ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL,
    };

    /// `0x4e5e20` walks the 120 slots and clears, each with a send of 0 (`0x4e5db0`), every slot
    /// whose spell `0x4e5a50` resolves to the unlearned one: a spell slot holding it, a macro slot
    /// whose cached spell it is whatever its book (`0x4e5ba0`), and an item slot whose on-use spell
    /// it is (`0x4e5ae0`). The rest stand.
    #[test]
    fn an_unlearn_clears_every_slot_that_stands_for_the_spell() {
        const FIREBALL: u32 = 133;
        const FROSTBOLT: u32 = 116;
        const GRENADE: u32 = 4390;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut actions = PlayerActions::default();
        for (slot, kind, action) in [
            (0u8, ACTION_KIND_SPELL, FIREBALL),
            (1, ACTION_KIND_SPELL, FROSTBOLT),
            (2, ACTION_KIND_MACRO, 1),
            (3, ACTION_KIND_MACRO, 2),
            (4, ACTION_KIND_ITEM, GRENADE),
            (5, ACTION_KIND_MACRO, 3),
        ] {
            actions
                .buttons
                .insert(slot, ActionButton { slot, action, kind });
        }
        actions.unlearned.push(FIREBALL);
        let mut bound = MacroBoundSpells::default();
        bound.0.insert(1, Bound::from(BoundSpell::Spell(FIREBALL)));
        bound.0.insert(2, Bound::from(BoundSpell::Spell(FROSTBOLT)));
        bound.0.insert(
            3,
            Bound {
                spell: BoundSpell::Spell(FIREBALL),
                pet_book: true,
            },
        );
        let mut items = Items::default();
        items.insert_template(
            GRENADE,
            Some(benilla_protocol::messages::ItemInfo {
                use_spell: Some(ItemUseSpell {
                    spell_id: FIREBALL,
                    cooldown_ms: -1,
                    category: 0,
                    category_cooldown_ms: -1,
                }),
                ..crate::items::test_template("Iron Grenade")
            }),
        );
        let mut app = App::new();
        app.insert_resource(actions)
            .insert_resource(bound)
            .insert_resource(items)
            .insert_resource(NetCommands(tx))
            .add_systems(Update, clear_unlearned_slots);
        app.update();

        let actions = app.world().resource::<PlayerActions>();
        let mut left: Vec<u8> = actions.buttons.keys().copied().collect();
        left.sort_unstable();
        assert_eq!(left, [1, 3]);
        assert!(
            actions.dirty,
            "the identity feed re-resolves the cleared slots"
        );
        assert!(actions.unlearned.is_empty());
        let sent: Vec<(u8, u32)> = rx
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::SetActionButton { button, packed } => Some((button, packed)),
                _ => None,
            })
            .collect();
        assert_eq!(sent, [(0, 0), (2, 0), (4, 0), (5, 0)], "lowest first");
    }
}
