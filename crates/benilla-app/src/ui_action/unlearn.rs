//! The unlearn's bar clear: `UnlearnSpell 0x5e9fe0` ends in `0x4e5e20` (`0x5ea253`), which clears
//! every slot that stands for the spell.

use bevy::prelude::*;

use benilla_protocol::messages::{ACTION_KIND_ITEM, ACTION_KIND_SPELL};
use benilla_ui::script::macro_slot_id;

use super::{ActionSlots, PlayerActions};
use crate::items::Items;
use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::Ask;
use crate::ui_macro::{BoundSpell, MacroBoundSpells};

/// `0x4e5e20` for each spell the removed-spell message unlearned ([`PlayerActions::unlearned`]):
/// the 120 slots in order, each whose spell `0x4e5a50` resolves to it cleared with a send of 0
/// (`0x4e5e40` into `0x4e5db0`). A spell slot resolves to its word, a macro slot to its cached
/// spell whatever its book (`0x4e5ba0`), an item slot to its template's on-use spell
/// (`0x4e5ae0`), none while the template is not cached. The identity feed then signals each
/// cleared slot.
pub(super) fn clear_unlearned_slots(
    mut actions: ResMut<PlayerActions>,
    mut slots: ResMut<ActionSlots>,
    bound: Res<MacroBoundSpells>,
    items: Res<Items>,
    commands: Res<NetCommands>,
) {
    if actions.unlearned.is_empty() {
        return;
    }
    let unlearned = std::mem::take(&mut actions.unlearned);
    for spell in unlearned {
        let mut cleared: Vec<u8> = slots
            .buttons
            .values()
            .filter(|b| {
                let resolved = match b.kind {
                    ACTION_KIND_SPELL => Some(b.action),
                    ACTION_KIND_ITEM => items
                        .template(b.action, 0, Ask::Read)
                        .and_then(|t| t.use_spell)
                        .map(|u| u.spell_id),
                    kind => macro_slot_id(kind, b.action)
                        .and_then(|id| bound.0.get(&id))
                        .and_then(|b| match b.spell {
                            BoundSpell::Spell(s) => Some(s),
                            _ => None,
                        }),
                };
                resolved == Some(spell)
            })
            .map(|b| b.slot)
            .collect();
        cleared.sort_unstable();
        for slot in cleared {
            slots.buttons.remove(&slot);
            slots.dirty = true;
            debug!("ui_action: unlearned spell {spell} clears wire slot {slot}");
            let _ = commands.0.send(ClientCommand::SetActionButton {
                button: slot,
                packed: 0,
            });
        }
    }
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
        let mut slots = crate::ui_action::ActionSlots::default();
        for (slot, kind, action) in [
            (0u8, ACTION_KIND_SPELL, FIREBALL),
            (1, ACTION_KIND_SPELL, FROSTBOLT),
            (2, ACTION_KIND_MACRO, 1),
            (3, ACTION_KIND_MACRO, 2),
            (4, ACTION_KIND_ITEM, GRENADE),
            (5, ACTION_KIND_MACRO, 3),
        ] {
            slots
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
            .insert_resource(slots)
            .insert_resource(bound)
            .insert_resource(items)
            .insert_resource(NetCommands(tx))
            .add_systems(Update, clear_unlearned_slots);
        app.update();

        let slots = app.world().resource::<ActionSlots>();
        let mut left: Vec<u8> = slots.buttons.keys().copied().collect();
        left.sort_unstable();
        assert_eq!(left, [1, 3]);
        assert!(
            slots.dirty,
            "the identity feed re-resolves the cleared slots"
        );
        assert!(app.world().resource::<PlayerActions>().unlearned.is_empty());
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

#[cfg(test)]
mod order_tests {
    use crate::game_plugins::schedule_tests::{
        census, headless_client, type_names_available, SyncPoints,
    };
    use bevy::ecs::schedule::SystemKey;
    use bevy::prelude::*;
    use std::collections::{HashMap, HashSet};

    /// The supersede's unlearn clears what still stands for the old rank after `0x4e5e60` moved the
    /// rest, so the clear runs after the rank pass, which stands in for that move on spell slots,
    /// and both bar writers run before the identity feed that consumes their `dirty`.
    #[test]
    fn the_unlearn_clear_runs_after_the_rank_pass_and_before_the_feed() {
        let mut app = headless_client();
        let c = census(&mut app, Update, SyncPoints::Declared);
        if !type_names_available(&c) {
            eprintln!(
                "skipped: this build carries no type names (bevy/debug rides with the dev feature)"
            );
            return;
        }
        let one = |suffix: &str| -> SystemKey {
            let mut hits = c.systems.iter().filter(|(_, s)| s.name.ends_with(suffix));
            match (hits.next(), hits.next()) {
                (Some((k, _)), None) => *k,
                _ => panic!("exactly one system named `…{suffix}` in Update"),
            }
        };
        let mut succ: HashMap<SystemKey, Vec<SystemKey>> = HashMap::new();
        for (a, b) in &c.dependencies {
            succ.entry(*a).or_default().push(*b);
        }
        let reach = |start: SystemKey| {
            let mut seen = HashSet::new();
            let mut stack = vec![start];
            while let Some(n) = stack.pop() {
                for m in succ.get(&n).into_iter().flatten() {
                    if seen.insert(*m) {
                        stack.push(*m);
                    }
                }
            }
            seen
        };
        let ranks = one("::ui_action::ranks::normalize_action_ranks");
        let clear = one("::ui_action::unlearn::clear_unlearned_slots");
        let feed = one("::ui_action::feed::feed_actions");
        assert!(
            reach(ranks).contains(&clear),
            "the clear runs after the rank pass"
        );
        assert!(reach(clear).contains(&feed), "and before the identity feed");
    }
}
