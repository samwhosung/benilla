//! The questgiver getters' reads at the call, the call host's half of
//! [`benilla_ui::script::QuestGiverQuery`]: the panel's cells as the packets left them
//! ([`super::QuestCells`]), each item row resolved off the item cache at the call (`0x5017de`,
//! `0x502114`), the reward spell off `Spell.dbc`, the background off the giver object
//! (`0x502230`) and `IsQuestCompletable`'s held counts off the player's items as of the drain's
//! place (`0x501230`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::messages::QuestRewardItem;
use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{QuestGiverQuery, QuestState};

use super::{resolve_items, reward_spell_view, QuestCells, QuestGiver};
use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::query_cache::Ask;
use crate::ui_action::Spells;
use crate::ui_items::{count_of, InventoryScope};

/// What the questgiver getters read as they run; the window absent in a harness without it, where
/// they read the VM's copy.
#[derive(SystemParam)]
pub(crate) struct QuestGiverLive<'w, 's> {
    giver: Option<Res<'w, QuestGiver>>,
    items: Option<Res<'w, Items>>,
    icons: Option<Res<'w, ItemDisplays>>,
    spells: Option<Res<'w, Spells>>,
    go_templates: Option<Res<'w, crate::go_templates::GameObjectTemplates>>,
    materials: Option<Res<'w, crate::ui_item_text::PageMaterials>>,
    objects: crate::net::Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
}

impl QuestGiverLive<'_, '_> {
    /// The cells as of the call.
    fn state(&self) -> Option<QuestState> {
        let (giver, items) = (self.giver.as_deref()?, self.items.as_deref()?);
        let material = giver.npc.and_then(|source| {
            crate::ui_item_text::object_material(
                source,
                &self.objects,
                items,
                self.go_templates.as_deref()?,
                self.materials.as_deref(),
            )
        });
        // The player's store as of here, and its items counted as the turn-in counts them
        // (`0x622130` with mask 8, `0x501275`).
        let player = self.objects.player().and_then(|e| self.stores.get(e).ok());
        let objects = self.stores.item_objects(&self.objects);
        let held = |entry| {
            player.map_or(0, |store| {
                count_of(&store.0, &objects, entry, InventoryScope::QUEST_ITEMS)
            })
        };
        Some(state(
            &giver.cells,
            items,
            self.icons.as_deref(),
            self.spells.as_deref(),
            material,
            player.is_some().then_some(&held as &dyn Fn(u32) -> u32),
        ))
    }
}

/// `cells` as the getters read them: each item row's record read without a callback, as the
/// getter's own lookup asks in its call, and the turn-in test of `0x501230`: the packet's flag,
/// a player (`held`), and each required row up to the first empty one held in its count.
pub(super) fn state(
    cells: &QuestCells,
    items: &Items,
    icons: Option<&ItemDisplays>,
    spells: Option<&Spells>,
    background_material: Option<String>,
    held: Option<&dyn Fn(u32) -> u32>,
) -> QuestState {
    let rows = |list: &[QuestRewardItem]| resolve_items(list, items, icons, Ask::Read);
    // No player answers nil (`0x50125f`); a held count below the row's is `jl` (`0x501286`).
    let completable = cells.state.completable
        && held.is_some_and(|held| {
            cells
                .required
                .iter()
                .take_while(|r| r.item_id != 0)
                .all(|r| held(r.item_id) as i32 >= r.count as i32)
        });
    QuestState {
        choices: rows(&cells.choices),
        rewards: rows(&cells.rewards),
        required: rows(&cells.required),
        reward_spell: reward_spell_view(cells.reward_spell, spells),
        completable,
        background_material,
        ..cells.state.clone()
    }
}

impl crate::script_calls::live::LiveFamily for QuestGiverLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|_: &QuestGiverQuery| self.state())
    }
}
