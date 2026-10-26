//! The item set block as the tooltip build reads it at the call, the call host's half of
//! [`benilla_ui::script::ItemSetQuery`]: the `ItemSet.dbc` row, its members off the item-name
//! cache and its bonuses expanded against the active player as they stand (`0x52d8a0..0x52e0f5`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::ItemSetQuery;

/// What the set block reads as it runs: our store as of the drain's place, which the bonuses'
/// `$`-tokens read. With the set table or the item-name cache absent it declines; with no active
/// player the build hides before it asks (`0x52b699`).
#[derive(SystemParam)]
pub(crate) struct ItemSetLive<'w, 's> {
    sets: Option<Res<'w, super::ItemSets>>,
    item_names: Option<Res<'w, crate::item_names::ItemNames>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    spell_mods: Option<Res<'w, crate::spell::SpellModifiers>>,
    skill_lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
    items: Option<Res<'w, crate::items::Items>>,
    objects: crate::net::Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
}

impl crate::script_calls::live::LiveFamily for ItemSetLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|q: &ItemSetQuery| {
            let (sets, item_names) = (self.sets.as_deref()?, self.item_names.as_deref()?);
            let me = self.objects.player().and_then(|e| self.stores.get(e).ok());
            Some(super::feed::item_set_live(
                q.0,
                sets,
                item_names,
                self.spells.as_deref(),
                self.spell_mods.as_deref(),
                self.skill_lines.as_deref(),
                self.items.as_deref(),
                me,
            ))
        })
    }
}
