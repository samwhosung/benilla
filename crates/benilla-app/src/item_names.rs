//! The item-name store (`0xc0e2dc`), apart from the item templates: `CMSG_ITEM_NAME_QUERY` asks
//! one item's name and `SMSG_ITEM_NAME_QUERY_RESPONSE` answers it (lookup `0x559fe0`, reader
//! `0x7c8dc0`). The reference reads an item set's member names through it (`0x52df5a`), never
//! through the template store, and saves it to `itemnamecache.wdb` ([`crate::wdb`]).

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands, NetHandlerApp};
use crate::query_cache::{Ask, QueryCache};

/// Item names by entry, asked once while in flight.
#[derive(Resource, Default)]
pub(crate) struct ItemNames {
    names: QueryCache<u32, String>,
}

impl ItemNames {
    /// The name of `entry`, read as `ask` says. The set tooltip's lookup passes a zero owner
    /// (`0x52df41`-`0x52df44`), so the query's guid is 0.
    pub(crate) fn name(&self, entry: u32, ask: Ask<'_>) -> Option<&str> {
        self.names
            .read(entry, ask, |commands: &NetCommands| {
                debug!("item names: asking item {entry}");
                let _ = commands
                    .0
                    .send(ClientCommand::ItemNameQuery { entry, guid: 0 });
            })
            .map(String::as_str)
    }

    /// Files an answer; `None` is the store's refusal (`0x556e67`).
    pub(crate) fn file(&mut self, entry: u32, name: Option<String>) {
        match name {
            Some(name) => self.names.insert(entry, name),
            None => {
                self.names.refuse(entry);
            }
        }
    }

    /// The name store, the one [`crate::wdb`] saves.
    pub(crate) fn store(&self) -> &QueryCache<u32, String> {
        &self.names
    }

    pub(crate) fn store_mut(&mut self) -> &mut QueryCache<u32, String> {
        &mut self.names
    }
}

pub(crate) struct ItemNamesPlugin;

impl Plugin for ItemNamesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ItemNames>()
            .net_handler(SessionEventKind::ItemName, on_item_name);
    }
}

/// `SMSG_ITEM_NAME_QUERY_RESPONSE`: the record lands, or the refusal frees the key.
fn on_item_name(In(ev): In<SessionEvent>, mut names: ResMut<ItemNames>) {
    if let SessionEvent::ItemName { entry, name } = ev {
        names.file(entry, name);
    }
}
