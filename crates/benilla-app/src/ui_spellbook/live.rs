//! The book getters' questions at the call, the call host's half of
//! [`benilla_ui::script::BookSlot`] and its kin: the book's order off the known spells
//! ([`super::book_order`]), each slot's record with its icon off the worn weapons
//! (`0x4b3f50`), and the checked ring off the form byte (`0x4b36d8`), the active player's store
//! read as of the drain's place.

use std::collections::BTreeSet;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{
    BookNow, BookSlot, BookSpell, BookTabs, SpellBookState, SpellCastUiQuery, SpellSlotView,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::ui_action::{PlayerActions, Spells};

/// The order the book had for the inputs it was last built from, which a getter reuses while they
/// stand: the known spells, the race and class, and the catalogs.
#[derive(Default)]
pub(crate) struct OrderMemo {
    key: Option<(BTreeSet<u32>, u8, u8, usize, usize)>,
    order: super::BookOrder,
}

/// What the book getters read as they run.
#[derive(SystemParam)]
pub(crate) struct BookLive<'w, 's> {
    actions: Option<Res<'w, PlayerActions>>,
    spells: Option<Res<'w, Spells>>,
    skill_lines: Option<Res<'w, super::SkillLines>>,
    objects: crate::net::Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    items: Option<Res<'w, Items>>,
    icons: Option<Res<'w, ItemDisplays>>,
    memo: Local<'s, OrderMemo>,
}

impl BookLive<'_, '_> {
    /// The active player's store as of here, `None` while none is held.
    fn player(&self) -> Option<&crate::net::ObjectStore> {
        self.objects.player().and_then(|e| self.stores.get(e).ok())
    }

    /// The book's order as it stands, `None` without the known spells or the catalog. Race and
    /// class are 0 before the descriptor, which keeps every spell in General.
    fn order(&mut self) -> Option<&super::BookOrder> {
        let (actions, spells) = (self.actions.as_deref()?, self.spells.as_deref()?);
        let (race, class) = self.player().map_or((0, 0), |s| {
            (s.0.unit_race().unwrap_or(0), s.0.unit_class().unwrap_or(0))
        });
        let lines = self.skill_lines.as_deref().map(|l| &l.catalog);
        let catalogs = (
            std::ptr::from_ref(&spells.catalog) as usize,
            lines.map_or(0, |l| std::ptr::from_ref(l) as usize),
        );
        let stands = self.memo.key.as_ref().is_some_and(|(known, r, c, s, l)| {
            (*r, *c, (*s, *l)) == (race, class, catalogs) && *known == actions.spells
        });
        if !stands {
            self.memo.order =
                super::book_order(&actions.spells, &spells.catalog, lines, race, class);
            self.memo.key = Some((actions.spells.clone(), race, class, catalogs.0, catalogs.1));
        }
        Some(&self.memo.order)
    }

    /// `spell_id`'s slot as it stands: its record, its icon off the worn weapons, the checked
    /// ring off the form byte (`0x4b3600`), and the face an action bar paints for it where it is
    /// not the book's (`0x4e6870`, `0x4e6bbd`). The cooldown the getters read off the list.
    fn slot(&self, spell_id: u32) -> SpellSlotView {
        let Some(spells) = self.spells.as_deref() else {
            return SpellSlotView::default();
        };
        let store = self.player();
        let icons = self.icons.as_deref();
        let Some(items) = self.items.as_deref() else {
            return super::slot_view(spell_id, &spells.catalog, || None, || None);
        };
        let objects = &self.objects;
        let mut slot = super::slot_view(
            spell_id,
            &spells.catalog,
            // The melee auto-attack shows the main-hand weapon, or Spell-Reset unarmed.
            || {
                store.map(|s| {
                    crate::ui_action::melee_auto_attack_icon(
                        s,
                        &spells.forms,
                        objects,
                        items,
                        icons,
                    )
                })
            },
            // Auto Shot and wand Shoot show the ranged weapon; without one, their own icon.
            || store.and_then(|s| crate::ui_action::ranged_weapon_icon(s, objects, items, icons)),
        );
        let Some(d) = spells.catalog.get(spell_id) else {
            return slot;
        };
        // The book's `IsCurrentCast` is `0x4b3600`, not the action bar's `0x4e53a0`: a form spell
        // is current while the form byte is its form; the open trade-skill arm is not built.
        let form = store.map_or(0, |s| s.0.unit_shapeshift_form());
        slot.current = form != 0 && d.shapeshift_form == Some(u32::from(form));
        if crate::ui_action::substitutes_weapon_icon(d) || d.active_icon_id != 0 {
            let face = crate::ui_action::spell_action_icon(
                spell_id, d, spells, store, objects, items, icons,
            );
            if face != slot.texture {
                slot.bar_texture = Some(face);
            }
        }
        slot
    }
}

impl crate::script_calls::live::LiveFamily for BookLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        if ask.answer(|_: &BookTabs| self.order().map(|o| o.tabs.clone())) {
            return true;
        }
        // An `enchant:` link's record test (`0x53222e`..`0x532243`): the row's `castUI`.
        if ask.answer(|q: &SpellCastUiQuery| {
            let spells = self.spells.as_deref()?;
            Some(spells.catalog.get(q.0).map(|d| d.cast_ui))
        }) {
            return true;
        }
        if ask.answer(|q: &BookSlot| {
            let id = self.order()?.slots.get(q.0).copied();
            Some(id.map(|id| self.slot(id)))
        }) {
            return true;
        }
        if ask.answer(|q: &BookSpell| {
            let held = self.order()?.slots.contains(&q.0);
            Some(held.then(|| self.slot(q.0)))
        }) {
            return true;
        }
        ask.answer(|_: &BookNow| {
            let order = self.order()?.clone();
            Some(SpellBookState {
                slots: order.slots.iter().map(|&id| self.slot(id)).collect(),
                tabs: order.tabs,
            })
        })
    }
}
