//! The inspect and honor getters' questions at the call, the call host's half of
//! [`benilla_ui::script::VisibleSlotQuery`], [`benilla_ui::script::HonorNow`] and
//! [`benilla_ui::script::InspectHonorNow`]: another player's visible-item block (`0x4c8328` →
//! `0x5f0d60`), the active player's honor block (`0x51a4b0`-`0x51aa90`) and the inspect-honor
//! slot (`0x4c95e0`, `0x4c9620`, `0x51ab00`), each read as the getter runs.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{HonorNow, HonorState, InspectHonorNow, InvSlotView, VisibleSlotQuery};

use super::VisibleItemTables;
use crate::net::{GuidIndex, SelfPlayer};

/// What the inspect and honor getters read as they run.
#[derive(SystemParam)]
pub(crate) struct InspectLive<'w, 's> {
    slot: Option<Res<'w, crate::ui_honor::InspectHonor>>,
    index: Option<Res<'w, GuidIndex>>,
    me: Query<'w, 's, Entity, With<SelfPlayer>>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    /// Absent in a harness without the item or name caches.
    tables: Option<VisibleItemTables<'w>>,
}

impl InspectLive<'_, '_> {
    /// Slot id `q.slot` of `q.guid`'s visible items as of here, `None` for an empty slot or no
    /// object; declined without the tables.
    fn visible(&self, q: &VisibleSlotQuery) -> Option<Option<InvSlotView>> {
        let tables = self.tables.as_ref()?;
        let entity = self.index.as_deref()?.0.get(&q.guid).copied();
        let Some(store) = self.stores.at(q.guid, entity) else {
            return Some(None);
        };
        Some(
            (1..=19)
                .contains(&q.slot)
                .then(|| tables.slot(store, q.slot - 1))
                .flatten(),
        )
    }

    /// The active player's honor block as of here; zeros with no active player (`0x51a524`).
    fn honor(&self) -> HonorState {
        self.me
            .iter()
            .next()
            .and_then(|e| self.stores.get(e).ok())
            .map(crate::ui_honor::honor_of)
            .unwrap_or_default()
    }
}

impl crate::script_calls::live::LiveFamily for InspectLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|q: &VisibleSlotQuery| self.visible(q))
            || ask.answer(|_: &HonorNow| Some(self.honor()))
            || ask.answer(|_: &InspectHonorNow| Some(self.slot.as_deref()?.slot()))
    }
}
