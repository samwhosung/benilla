//! The skills pane's one send, the abandon; skills are granted server-side as field updates.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_SKILL_BUY_STEP`: `BuySkillTier`.
    pub fn skill_buy_step(&mut self, skill_id: u32) -> Result<()> {
        self.send(
            opcode::CMSG_SKILL_BUY_STEP,
            &messages::skill_buy_step(skill_id),
        )
    }

    /// `CMSG_UNLEARN_SKILL`: no ack; the removal comes back as a `PLAYER_SKILL_INFO` update.
    pub fn unlearn_skill(&mut self, skill_id: u32) -> Result<()> {
        self.send(
            opcode::CMSG_UNLEARN_SKILL,
            &messages::unlearn_skill(skill_id),
        )
    }
}
