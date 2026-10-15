//! Skill-line messages: the skills pane's `CMSG_UNLEARN_SKILL` (0x202). `HandleUnlearnSkillOpcode`
//! drops and anticheat-flags a request for a line without `SKILL_FLAG_UNLEARNABLE` (0x20) in its
//! `SkillRaceClassInfo.flags`; a removal comes back as a `PLAYER_SKILL_INFO` update, not an ack.

/// Body of `CMSG_SKILL_BUY_STEP` (`0x21f`, pushed at `0x4d3fbc`): the skill line id (`0x4d3fe0`).
pub fn skill_buy_step(skill_id: u32) -> Vec<u8> {
    skill_id.to_le_bytes().to_vec()
}

/// Body of `CMSG_UNLEARN_SKILL`: one `u32` skill line id.
pub fn unlearn_skill(skill_id: u32) -> Vec<u8> {
    skill_id.to_le_bytes().to_vec()
}
