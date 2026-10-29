//! The cooldown getters' query at the call, the call host's half of
//! [`benilla_ui::script::CooldownQuery`]: the player's list and the pet's as of the drain's place
//! (their cooldown lanes replay them packet by packet, [`super::log`]), each item found as of there too
//! ([`crate::ui_unit::views::ViewStores`]), each answer derived as the feeds derive their
//! snapshots.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{CooldownQuery, CooldownTriple};

use super::Cooldowns;
use crate::query_cache::Ask;

/// What the cooldown getters read as they run.
#[derive(SystemParam)]
pub(crate) struct CooldownLive<'w, 's> {
    cooldowns: Option<Res<'w, Cooldowns>>,
    pet: Option<Res<'w, crate::ui_pet::PetBar>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    items: Option<Res<'w, crate::items::Items>>,
    /// The interface clock's pair every start is mapped through, as the pushes map theirs.
    clock: Option<Res<'w, crate::ui_script::HostClock>>,
    objects: crate::net::Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
}

impl CooldownLive<'_, '_> {
    /// `query`'s answer as of here; `None` for an item object not held then, and in a harness
    /// without the list, the items or the clock.
    pub(crate) fn read(&self, query: CooldownQuery) -> Option<Option<CooldownTriple>> {
        let clock = &self.clock.as_deref()?.0;
        let (anchor, ui_now) = (clock.anchor, clock.ui_now);
        let catalog = self.spells.as_deref().map(|s| &s.catalog);
        // `0x6e2ea0` on list 1 with item 0, as the pet bar's feed reads it.
        if let CooldownQuery::PetSpell(spell) = query {
            return Some(
                self.pet
                    .as_deref()?
                    .cooldowns
                    .info(
                        spell,
                        0,
                        catalog.and_then(|c| c.get(spell)),
                        std::time::Instant::now(),
                    )
                    .ui_triple(anchor, ui_now),
            );
        }
        let (cooldowns, items) = (self.cooldowns.as_deref()?, self.items.as_deref()?);
        let objects = self.stores.item_objects(&self.objects);
        match query {
            // `0x6e2ea0`: the spell with item 0, as the bar's and the book's pushes read it.
            CooldownQuery::Spell(spell) => Some(
                cooldowns
                    .info(
                        spell,
                        0,
                        catalog.and_then(|c| c.get(spell)),
                        std::time::Instant::now(),
                    )
                    .ui_triple(anchor, ui_now),
            ),
            // `0x6e2ed0` by entry: the record's first on-use spell with the item (`0x6e2f20`),
            // the record only read here; one not cached queries nothing (`0x6e2f14`).
            CooldownQuery::Item(entry) => {
                let Some(u) = items
                    .template(entry, 0, Ask::Read)
                    .and_then(|t| t.use_spell)
                else {
                    return Some(None);
                };
                let d = catalog.and_then(|c| c.get(u.spell_id));
                Some(
                    cooldowns
                        .info_item(u.spell_id, entry, u.category, d, std::time::Instant::now())
                        .ui_triple(anchor, ui_now),
                )
            }
            CooldownQuery::ItemObject(guid) => crate::ui_items::feed::object_cooldown(
                &objects, guid, items, cooldowns, catalog, anchor, ui_now,
            ),
            // The active player's worn slot as of here (`0x4c8b10` → `0x6228a0`).
            CooldownQuery::Worn(slot0) => {
                let Some(guid) = self
                    .objects
                    .player()
                    .and_then(|e| self.stores.get(e).ok())
                    .and_then(|s| s.0.player_inv_slot(slot0))
                    .filter(|&g| g != 0)
                else {
                    return Some(None);
                };
                Some(
                    crate::ui_items::feed::object_cooldown(
                        &objects, guid, items, cooldowns, catalog, anchor, ui_now,
                    )
                    .flatten(),
                )
            }
            CooldownQuery::PetSpell(_) => None,
        }
    }
}

impl crate::script_calls::live::LiveFamily for CooldownLive<'_, '_> {
    fn answer(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
        ask.answer(|q: &CooldownQuery| self.read(*q))
    }
}
