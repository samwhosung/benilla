//! The name caches' reads at the call, the call host's half of
//! [`benilla_ui::script::PlayerNameQuery`] and [`benilla_ui::script::UnitRecordQuery`]: the
//! player-name cache's record read only (`0x55f080`), and the creature-record legs of a held
//! unit, each as the snapshot's build derives them ([`crate::ui_unit::snapshot`],
//! [`crate::ui_unit::enrich_unit`]) off the unit object as of the drain's place.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{PlayerNameQuery, UnitRecord, UnitRecordQuery};

use super::NameCache;
use crate::creature_type::CreatureTypeSources;
use crate::query_cache::Ask;

/// What the name getters read as they run.
#[derive(SystemParam)]
pub(crate) struct NameLive<'w, 's> {
    names: Option<Res<'w, NameCache>>,
    index: Option<Res<'w, crate::net::GuidIndex>>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    /// The form table and the type words the creature type resolves through.
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    words: Option<Res<'w, crate::target::CreatureTypes>>,
}

impl NameLive<'_, '_> {
    /// The player-name cache's record for `guid`, read only: `Some(None)` for a miss or a refusal.
    /// Declined in a harness without the cache.
    fn player(&self, guid: u64) -> Option<Option<String>> {
        let names = self.names.as_deref()?;
        Some(names.resolve_player(guid, Ask::Read).map(str::to_string))
    }

    /// The creature-record legs of the unit held under `guid` as of here (`0x605570`, `0x605620`,
    /// `0x612550`): a player has no record to read (the snapshot reads none either), and a
    /// creature without its template cached reads rank 0, no civilian flag, and its race's type.
    /// Declined for no unit held and in a harness without the cache.
    fn record(&self, guid: u64) -> Option<UnitRecord> {
        let names = self.names.as_deref()?;
        let entity = self.index.as_deref().and_then(|i| i.0.get(&guid).copied());
        let store = self.stores.at(guid, entity).filter(|s| s.is_unit())?;
        let types =
            CreatureTypeSources::of_resources(names, self.spells.as_deref(), self.words.as_deref());
        let creature_type = types.word(types.of(store)).map(str::to_string);
        if benilla_protocol::guid::is_player(guid) {
            return Some(UnitRecord {
                creature_type,
                ..Default::default()
            });
        }
        // Keyed by the unit's own `OBJECT_FIELD_ENTRY`, an entry of 0 being no key (`0x604629`).
        let record = store
            .0
            .object_entry()
            .filter(|&e| e != 0)
            .and_then(|e| names.creature_record(e));
        Some(UnitRecord {
            creature_type,
            rank: super::gated_rank(record, Some(store)),
            civilian: record.is_some_and(|r| r.civilian),
        })
    }
}

impl crate::script_calls::live::LiveFamily for NameLive<'_, '_> {
    fn answer(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
        ask.answer(|q: &PlayerNameQuery| self.player(q.0))
            || ask.answer(|q: &UnitRecordQuery| self.record(q.0))
    }
}
