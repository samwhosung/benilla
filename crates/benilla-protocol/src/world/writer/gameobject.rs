//! The GameObject sends; one `CMSG_GAMEOBJ_USE` serves every kind, dispatched by type.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// Use a world GameObject; the server answers by type or refuses silently, with no ack.
    pub fn gameobj_use(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_GAMEOBJ_USE, &messages::gameobj_use(guid))
    }

    /// Ask a GameObject template; `entry` is the object's `OBJECT_FIELD_ENTRY`.
    pub fn gameobject_query(&mut self, entry: u32, guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_GAMEOBJECT_QUERY,
            &messages::gameobject_query(entry, guid),
        )
    }

    /// Ask a book page; the server resolves it by id alone and answers each page of the chain.
    pub fn page_text_query(&mut self, page_id: u32, guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_PAGE_TEXT_QUERY,
            &messages::page_text_query(page_id, guid),
        )
    }
}
