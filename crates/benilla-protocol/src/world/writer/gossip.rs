//! The gossip sends. `CMSG_GOSSIP_HELLO` opens every NPC service window: vmangos accepts it for any
//! interactable creature (`GetNPCIfCanInteractWith` with `UNIT_NPC_FLAG_NONE`,
//! `NPCHandler.cpp:347`).

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// Open a gossip menu on an NPC (`CMSG_GOSSIP_HELLO`), answered by `SMSG_GOSSIP_MESSAGE`.
    pub fn gossip_hello(&mut self, npc_guid: u64) -> Result<()> {
        self.send(opcode::CMSG_GOSSIP_HELLO, &messages::gossip_hello(npc_guid))
    }

    /// Choose a gossip option by its echoed `index`; `code` is sent only for a coded option.
    pub fn gossip_select_option(
        &mut self,
        npc_guid: u64,
        gossip_list_id: u32,
        code: Option<&str>,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_GOSSIP_SELECT_OPTION,
            &messages::gossip_select_option(npc_guid, gossip_list_id, code),
        )
    }

    /// Ask a gossip menu's greeting text (`CMSG_NPC_TEXT_QUERY`); ask once and cache.
    pub fn npc_text_query(&mut self, text_id: u32, guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_NPC_TEXT_QUERY,
            &messages::npc_text_query(text_id, guid),
        )
    }
}
