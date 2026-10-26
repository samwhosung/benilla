//! The selection's and the interaction's globals at the call, the call host's half of
//! [`benilla_ui::script::SelectionQuery`]: the resolver reads the target global `[0xb4e2d8]`
//! (`0x515a86`) and the interaction global `[0xb4e2d0]` (`0x515c00`) when the getter runs, so a
//! listener reads the selection its own packet's handler left; and the name `UnitName` gives an
//! object held under a guid that is no unit ([`ObjectNameQuery`]).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{ObjectNameQuery, SelectionGlobals, SelectionQuery};

use super::Selection;
use crate::net::{GuidIndex, ObjectStore};

/// What `"target"` and `"npc"` resolve through as a getter runs.
#[derive(SystemParam)]
pub(crate) struct SelectionLive<'w, 's> {
    selection: Option<Res<'w, Selection>>,
    latch: Option<Res<'w, crate::ui_session::InteractLatch>>,
    /// The objects, and the caches an object that is no unit is named from.
    index: Option<Res<'w, GuidIndex>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    go_templates: Option<Res<'w, crate::go_templates::GameObjectTemplates>>,
    items: Option<Res<'w, crate::items::Items>>,
}

impl SelectionLive<'_, '_> {
    /// The two globals as of here; `None` in a harness without the selection or the latch, whose
    /// getters read the snapshot.
    fn globals(&self) -> Option<SelectionGlobals> {
        let selection = self.selection.as_deref()?;
        let latch = self.latch.as_deref()?;
        Some(SelectionGlobals {
            target: selection.guid.unwrap_or(0),
            npc: latch.guid().unwrap_or(0),
        })
    }

    /// [`ObjectNameQuery`]'s answer: `OBJECT_FIELD_TYPE`'s unit bit first (`0x517126`), then the
    /// game object's (`0x51714b`), then the item's (`0x51715d`), each object as of here.
    fn object_name(&self, guid: u64) -> Option<Option<String>> {
        let entity = self.index.as_deref()?.0.get(&guid).copied()?;
        let fields = &self.stores.get(entity).ok()?.0;
        let mask = fields.object_type_mask()?;
        let entry = fields.object_entry().unwrap_or(0);
        if mask & 0x8 != 0 {
            None
        } else if mask & 0x20 != 0 {
            let name = self.go_templates.as_deref()?.name(entry);
            Some(Some(name.unwrap_or_default().to_string()))
        } else if mask & 0x2 != 0 {
            let items = self.items.as_deref()?;
            Some(
                items
                    .template(entry, guid, crate::query_cache::Ask::Read)
                    .map(|t| t.name.clone()),
            )
        } else {
            None
        }
    }
}

impl crate::script_calls::live::LiveFamily for SelectionLive<'_, '_> {
    fn answer(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
        ask.answer(|_: &SelectionQuery| self.globals())
            || ask.answer(|q: &ObjectNameQuery| self.object_name(q.0))
    }
}
