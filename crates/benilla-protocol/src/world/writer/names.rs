//! The ask-once lookups that turn a guid into a player, creature or pet name.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_NAME_QUERY`: a player's name, race, gender and class, by full 8-byte guid.
    pub fn name_query(&mut self, guid: u64) -> Result<()> {
        self.send(opcode::CMSG_NAME_QUERY, &messages::full_guid(guid))
    }

    /// `CMSG_CREATURE_QUERY`: a template's name and subname; `entry` is the creature template entry.
    pub fn creature_query(&mut self, entry: u32, guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_CREATURE_QUERY,
            &messages::creature_query(entry, guid),
        )
    }

    /// `CMSG_PET_NAME_QUERY`: the only way to name a pet, whose guid holds a pet number, not an
    /// entry. A pet that is gone gets no answer.
    pub fn pet_name_query(&mut self, pet_number: u32, guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_PET_NAME_QUERY,
            &messages::pet_name_query(pet_number, guid),
        )
    }
}
