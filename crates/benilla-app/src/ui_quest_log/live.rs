//! The quest log getters' questions at the call, the call host's half of
//! [`benilla_ui::script::QuestSlots`] and its kin: the player's slots and coinage off the store
//! as of the drain's place (`0x4dfa41`, `0x4e01df`, `0x4df70b`), the held count of an item with
//! each item found as of there (`0x622130`), a faction's standing off the reputation slots as
//! the reputation lane replays them (`0x4d6370`), the clock the countdowns read, whether the
//! player is held (`0x4de568`), a creature's and an object's name off their caches (`0xc0e354`,
//! `0xc0e318`), and the quest text's `$` macros expanded against the player as of there and the
//! world states (`0x506f70`, `0x4dff89`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{
    QuestClock, QuestClockRead, QuestCoinage, QuestCreatureName, QuestHeld, QuestObjectName,
    QuestPlayer, QuestSlots, QuestStanding, QuestText, SelfRead,
};

use crate::net::{Reputations, ServerWallClock};
use crate::query_cache::Ask;
use crate::target::Factions;
use crate::ui_items::{count_of, InventoryScope};

/// What the quest log getters read as they run.
#[derive(SystemParam)]
pub(crate) struct QuestLogLive<'w, 's> {
    objects: crate::net::Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    reputations: Option<Res<'w, Reputations>>,
    factions: Option<Res<'w, Factions>>,
    clock: Option<Res<'w, ServerWallClock>>,
    go_templates: Option<Res<'w, crate::go_templates::GameObjectTemplates>>,
    names: Option<Res<'w, crate::names::NameCache>>,
    me: Option<Res<'w, crate::net::SelfGuid>>,
    states: Option<Res<'w, crate::world_state::WorldStates>>,
    unit_names: crate::ui_unit::NameTables<'w>,
}

impl QuestLogLive<'_, '_> {
    /// The active player's store as of here, `None` while none is held.
    fn player(&self) -> Option<&crate::net::ObjectStore> {
        self.objects.player().and_then(|e| self.stores.get(e).ok())
    }

    /// `faction`'s standing as of here: `0x4d6370`'s base for the player's race and class plus the
    /// wire's slot, 0 for a faction with no reputation index. `None` without the player, the
    /// slots or the catalog, where the snapshot answers.
    fn standing(&self, faction: u32) -> Option<i32> {
        let (reputations, factions) = (self.reputations.as_deref()?, self.factions.as_deref()?);
        let store = self.player()?;
        Some(crate::ui_reputation::faction_standing(
            factions.catalog(),
            reputations,
            store.0.unit_race().unwrap_or(0),
            store.0.unit_class().unwrap_or(0),
            faction,
        ))
    }

    /// `details` and `objectives` expanded as `ExpandServerText` expands them (`0x506f70`): against
    /// the player as of here, named off the name cache read without a callback (`0x506fd4`), and
    /// the world states. `None` without the cache or the states, where the record's text answers.
    fn text(&self, q: &QuestText) -> Option<(String, String)> {
        let (names, states) = (self.names.as_deref()?, self.states.as_deref()?);
        let guid = self.me.as_deref().and_then(|me| me.0);
        let subject = guid.zip(self.player()).and_then(|(guid, store)| {
            crate::npc_text::player_subject(guid, store, names, self.unit_names.get())
        });
        let macros = crate::npc_text::MacroContext {
            subject: subject.as_ref(),
            states,
        };
        Some((
            crate::npc_text::substitute(&q.details, &macros),
            crate::npc_text::substitute(&q.objectives, &macros),
        ))
    }

    /// `0x4deef0`'s clock as it stands, `None` for a harness without one.
    fn clock(&self) -> Option<QuestClockRead> {
        let clock = self.clock.as_deref()?;
        Some(QuestClockRead {
            server: clock.now_unix(),
            zero: clock.zero_offset(),
            local: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs_f64()),
        })
    }
}

impl crate::script_calls::live::LiveFamily for QuestLogLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|_: &QuestSlots| self.player().map(|s| super::slot_views(&s.0)))
            || ask.answer(|q: &QuestHeld| {
                let store = self.player()?;
                let items = self.stores.item_objects(&self.objects);
                Some(count_of(&store.0, &items, q.0, InventoryScope::QUEST_ITEMS))
            })
            || ask.answer(|q: &QuestStanding| self.standing(q.0))
            || ask.answer(|_: &QuestCoinage| {
                self.player()
                    .map(|s| crate::ui_unit::StoreObject(s).money())
            })
            || ask.answer(|_: &QuestClock| self.clock())
            || ask.answer(|q: &QuestObjectName| {
                Some(self.go_templates.as_deref()?.name(q.0)?.to_string())
            })
            || ask.answer(|q: &QuestCreatureName| {
                let names = self.names.as_deref()?;
                Some(names.resolve_creature(q.0, 0, Ask::Read)?.to_string())
            })
            || ask.answer(|_: &QuestPlayer| self.player().map(|_| true))
            || ask.answer(|q: &QuestText| self.text(q))
    }
}
