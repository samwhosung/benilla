//! The reader's getters' reads at the call, the call host's half of
//! [`benilla_ui::script::ItemTextQuery`]: the reader's cells as the session holds them (the page
//! `0xbc3fc4`, the page list `0xbc3fd0`, the text `0xbc3fe8`) and the open object resolved as the
//! getters resolve it as they run: its name (`0x4e38f0`, any type, `vtbl+0x70`), its creator off
//! the player-name cache (`0x4e3940`, an item only) and its material (`0x4e39f0`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{ItemTextCreator, ItemTextQuery, ItemTextView};

use super::{ItemTextOpen, PageMaterials};
use crate::go_templates::GameObjectTemplates;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::Objects;
use crate::query_cache::Ask;

/// What the reader's getters read as they run; absent in a harness without the reader or the
/// objects, where they read the VM's copy.
#[derive(SystemParam)]
pub(crate) struct ItemTextLive<'w, 's> {
    open: Res<'w, ItemTextOpen>,
    objects: Objects<'w, 's>,
    items: Res<'w, Items>,
    go_templates: Res<'w, GameObjectTemplates>,
    materials: Option<Res<'w, PageMaterials>>,
    names: Res<'w, NameCache>,
}

impl ItemTextLive<'_, '_> {
    /// The reader now. The cells read as an open empties them (`0x4e3372`-`0x4e338f`) with no
    /// read open.
    fn view(&self) -> ItemTextView {
        let Some(sess) = self.open.pending.as_ref() else {
            return ItemTextView {
                page: 1,
                ..ItemTextView::default()
            };
        };
        let object = sess.object_guid;
        let item = super::readable(object, &self.objects, &self.items, &self.go_templates)
            .map(|r| r.title);
        let material = super::object_material(
            object,
            &self.objects,
            &self.items,
            &self.go_templates,
            self.materials.as_deref(),
        );
        ItemTextView {
            item,
            creator: self.creator(object),
            text: sess.text.clone(),
            page: sess.page_number(),
            has_next: sess.has_next(),
            material,
        }
    }

    /// The open item's creator (`0x4e3940`): an item only (mask 2, `0x4e3974`), its
    /// `ITEM_FIELD_CREATOR` (`+0x10`, `0x4e3988`), named off the player-name cache alone
    /// (`0xc0e228`); a name the cache lacks reads nil and takes the miss's callback.
    fn creator(&self, object: u64) -> ItemTextCreator {
        let Some(guid) = self
            .objects
            .object(object)
            .filter(|f| f.is_item())
            .and_then(|f| f.item_creator())
            .filter(|g| *g != 0)
        else {
            return ItemTextCreator::Nobody;
        };
        match self.names.resolve_player(guid, Ask::Read) {
            Some(name) => ItemTextCreator::Named(name.to_string()),
            None => ItemTextCreator::Unnamed(guid),
        }
    }
}

impl crate::script_calls::live::LiveFamily for ItemTextLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|_: &ItemTextQuery| Some(self.view()))
    }
}
