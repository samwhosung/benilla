//! The sends behind the console's own commands (`messages::console_commands`).

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `port x y [z] [facing]` (`0x404cdc`).
    pub fn teleport_cheat(&mut self, pos: [f32; 3], facing: f32) -> Result<()> {
        self.send(
            opcode::MSG_MOVE_TELEPORT_CHEAT,
            &messages::raw_position(pos, facing),
        )
    }

    /// `port <name>` (`0x404b31`): the argument text as it stands.
    pub fn teleport_to_unit(&mut self, text: &str) -> Result<()> {
        self.send(
            opcode::CMSG_TELEPORT_TO_UNIT,
            &messages::teleport_to_unit(text),
        )
    }

    /// `setrawpos x y [z] [facing]` (`0x4051e4`).
    pub fn set_raw_position(&mut self, pos: [f32; 3], facing: f32) -> Result<()> {
        self.send(
            opcode::CMSG_MOVE_SET_RAW_POSITION,
            &messages::raw_position(pos, facing),
        )
    }

    /// `worldport <map> [x y z] [facing]` (`0x404fc8`).
    pub fn world_teleport(
        &mut self,
        time_ms: u32,
        map: u32,
        pos: [f32; 3],
        facing: f32,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_WORLD_TELEPORT,
            &messages::world_teleport(time_ms, map, pos, facing),
        )
    }

    /// `whois <name>` (`0x5adf1f`): the argument text as it stands.
    pub fn whois(&mut self, text: &str) -> Result<()> {
        self.send(opcode::CMSG_WHOIS, &messages::whois(text))
    }
}
