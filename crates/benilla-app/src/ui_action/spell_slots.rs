//! The bar's legs of the spell packets, run inside their handlers: the unlearn's `ClearSpellSlots`
//! `0x4e5e20` (from `0x5ea253`) and the supersede's `ReplaceActionSpell` `0x4e5e60` (from
//! `0x5e6356`). Each slot write is `ClearActionSlot 0x4e5db0` or `SetActionSlot 0x4e5d60` into
//! `0x4e58e0`: the word stored, with the VM's copy of the slot, `CMSG_SET_ACTION_BUTTON` sent,
//! then `ACTIONBAR_SLOT_CHANGED` with the 1-based slot on the notice lane, at the packet's place.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::messages::{ActionButton, ACTION_KIND_ITEM, ACTION_KIND_SPELL};
use benilla_ui::script::{macro_slot_id, ActionSlot, UiScript};

use super::{ActionSlots, Spells};
use crate::entities::ItemDisplays;
use crate::field_watch::{FieldNotice, NoticeLane};
use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::query_cache::Ask;

/// What the bar's legs of a spell packet's handler write and read: the slots, the VM's copy of
/// them and its macro records, and what a spell slot's icon resolves through.
#[derive(SystemParam)]
pub(crate) struct SpellSlots<'w, 's> {
    slots: ResMut<'w, ActionSlots>,
    items: Res<'w, Items>,
    net: Res<'w, NetCommands>,
    script: Option<NonSendMut<'w, UiScript>>,
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    objects: Objects<'w, 's>,
    icons: Option<Res<'w, ItemDisplays>>,
}

impl SpellSlots<'_, '_> {
    /// Whether our player is held, whose unlearn and learn a supersede runs (`0x5e6385`).
    pub(crate) fn player_held(&self) -> bool {
        !self.me.is_empty()
    }

    /// `ClearSpellSlots 0x4e5e20(spell)`: the 120 slots in order, each whose spell
    /// `GetActionSlotSpell 0x4e5a50` resolves to `spell` cleared with a send of 0 (`0x4e5e40` into
    /// `0x4e5db0`). A spell slot resolves to its word, a macro slot to its record's cached spell
    /// whatever its book (`0x4e5ba0`), an item slot to its template's on-use spell (`0x4e5ae0`),
    /// none while the template is not cached.
    pub(crate) fn clear_spell(&mut self, spell: u32, lane: &mut NoticeLane) {
        let records = self.macro_records();
        let mut cleared: Vec<u8> = self
            .slots
            .buttons
            .values()
            .filter(|b| self.slot_spell(b, &records) == Some(spell))
            .map(|b| b.slot)
            .collect();
        cleared.sort_unstable();
        for slot in cleared {
            debug!("ui_action: unlearned spell {spell} clears wire slot {slot}");
            self.write(slot, None, None, lane);
        }
    }

    /// `ReplaceActionSpell 0x4e5e60(old, new)`: the 120 slots in order. A slot whose spell
    /// (`0x4e5a50`) is `old` and whose word is a spell's (`0x4e5e89`) is cleared with a send, then
    /// set to `new` with a send (`0x4e5e93`, `0x4e5ea0`), each signalled; one whose word is a
    /// macro's gets `new` in its record's cached spell (`0x4e5ec4`), with no signal. An item slot
    /// is left as it is.
    pub(crate) fn replace_spell(
        &mut self,
        old: u32,
        new: u32,
        spells: Option<&Spells>,
        lane: &mut NoticeLane,
    ) {
        let mut records = self.macro_records();
        let mut matched: Vec<ActionButton> = self
            .slots
            .buttons
            .values()
            .filter(|b| self.slot_spell(b, &records) == Some(old))
            .copied()
            .collect();
        matched.sort_unstable_by_key(|b| b.slot);
        let mut rebound = false;
        for b in matched {
            if b.kind & 0xf0 == ACTION_KIND_SPELL {
                debug!(
                    "ui_action: superseded spell {old} -> {new} in wire slot {}",
                    b.slot
                );
                self.write(b.slot, None, spells, lane);
                let word = ActionButton {
                    slot: b.slot,
                    action: new,
                    kind: ACTION_KIND_SPELL,
                };
                self.write(b.slot, Some(word), spells, lane);
            } else if let Some(id) = macro_slot_id(b.kind, b.action) {
                if let Some(record) = records.as_mut().and_then(|r| r.get_mut(&id)) {
                    record.spell = new as i32;
                    rebound = true;
                }
            }
        }
        if let (true, Some(records), Some(script)) = (rebound, records, self.script.as_deref_mut())
        {
            script.set_macro_bindings(records);
        }
    }

    /// The VM's macro records, the reference's own (`[rec+0x564]`); `None` with no VM, whose
    /// interface holds no macro table.
    fn macro_records(
        &self,
    ) -> Option<std::collections::HashMap<u32, benilla_ui::script::MacroBinding>> {
        let script = self.script.as_deref()?;
        let macros = script.macros();
        let mut records = script.macro_bindings();
        records.retain(|id, _| macros.by_id(*id).is_some());
        Some(records)
    }

    /// `GetActionSlotSpell 0x4e5a50` for `b`: a spell word's own spell, an item's on-use spell
    /// (`0x4e5ae0`), a macro's cached spell when the macro exists (`0x4e5ba0`).
    fn slot_spell(
        &self,
        b: &ActionButton,
        records: &Option<std::collections::HashMap<u32, benilla_ui::script::MacroBinding>>,
    ) -> Option<u32> {
        match b.kind {
            ACTION_KIND_SPELL => Some(b.action),
            ACTION_KIND_ITEM => self
                .items
                .template(b.action, 0, Ask::Read)
                .and_then(|t| t.use_spell)
                .map(|u| u.spell_id),
            kind => {
                let id = macro_slot_id(kind, b.action)?;
                u32::try_from(records.as_ref()?.get(&id)?.spell).ok()
            }
        }
    }

    /// One slot's `0x4e58e0` with its send: `word` stored (`None` stores 0), the VM's copy of the
    /// slot with it, the send, then `ACTIONBAR_SLOT_CHANGED` on the lane, whose firing recomputes
    /// the slot's usable pair first (`0x4e5903`).
    fn write(
        &mut self,
        slot: u8,
        word: Option<ActionButton>,
        spells: Option<&Spells>,
        lane: &mut NoticeLane,
    ) {
        let packed = word.map_or(0, |w| w.action | (u32::from(w.kind) << 24));
        match word {
            Some(w) => {
                self.slots.buttons.insert(slot, w);
            }
            None => {
                self.slots.buttons.remove(&slot);
            }
        }
        let action = u32::from(slot) + 1;
        let shown = word.map(|w| self.spell_slot(w, spells));
        if let Some(script) = self.script.as_deref_mut() {
            script.set_action(action, shown);
        }
        let _ = self.net.0.send(ClientCommand::SetActionButton {
            button: slot,
            packed,
        });
        lane.write(FieldNotice::ActionSlotChanged { action });
    }

    /// The VM's copy of a spell word, its icon resolved ([`super::feed::spell_action_icon`]) for a
    /// getter with no host lent; with one, the getter resolves it at the call.
    fn spell_slot(&self, w: ActionButton, spells: Option<&Spells>) -> ActionSlot {
        let texture = spells.and_then(|sp| {
            let d = sp.catalog.get(w.action)?;
            super::spell_action_icon(
                w.action,
                d,
                sp,
                self.me.iter().next(),
                &self.objects,
                &self.items,
                self.icons.as_deref(),
            )
        });
        ActionSlot {
            texture,
            kind: w.kind,
            action: w.action,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_macro::{Bound, BoundSpell};
    use benilla_protocol::messages::{ItemUseSpell, ACTION_KIND_MACRO};
    use bevy::ecs::system::RunSystemOnce;

    /// `0x4e5e20` walks the 120 slots and clears, each with a send of 0 (`0x4e5db0`) and its
    /// signal, every slot whose spell `0x4e5a50` resolves to the unlearned one: a spell slot holding
    /// it, a macro slot whose cached spell it is whatever its book (`0x4e5ba0`), and an item slot
    /// whose on-use spell it is (`0x4e5ae0`). The rest stand.
    #[test]
    fn an_unlearn_clears_every_slot_that_stands_for_the_spell() {
        const FIREBALL: u32 = 133;
        const FROSTBOLT: u32 = 116;
        const GRENADE: u32 = 4390;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut slots = ActionSlots::default();
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
        let mut script = UiScript::new().unwrap();
        crate::ui_macro::bind_for_test(
            &mut script,
            &[
                (1, Bound::from(BoundSpell::Spell(FIREBALL))),
                (2, Bound::from(BoundSpell::Spell(FROSTBOLT))),
                (
                    3,
                    Bound {
                        spell: BoundSpell::Spell(FIREBALL),
                        pet_book: true,
                    },
                ),
            ],
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
        let mut world = World::new();
        world.insert_resource(slots);
        world.insert_non_send_resource(script);
        world.insert_resource(items);
        world.insert_resource(NetCommands(tx));
        world.init_resource::<crate::net::GuidIndex>();
        world.init_resource::<NoticeLane>();
        world
            .run_system_once(|mut bar: SpellSlots, mut lane: ResMut<NoticeLane>| {
                bar.clear_spell(FIREBALL, &mut lane)
            })
            .expect("the clear runs");

        let slots = world.resource::<ActionSlots>();
        let mut left: Vec<u8> = slots.buttons.keys().copied().collect();
        left.sort_unstable();
        assert_eq!(left, [1, 3]);
        let sent: Vec<(u8, u32)> = rx
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::SetActionButton { button, packed } => Some((button, packed)),
                _ => None,
            })
            .collect();
        assert_eq!(sent, [(0, 0), (2, 0), (4, 0), (5, 0)], "lowest first");
        let signalled: Vec<FieldNotice> = world.resource::<NoticeLane>().iter().copied().collect();
        assert_eq!(
            signalled,
            [1, 3, 5, 6].map(|action| FieldNotice::ActionSlotChanged { action }),
            "each clear signals its 1-based slot, in the walk's order"
        );
    }
}
