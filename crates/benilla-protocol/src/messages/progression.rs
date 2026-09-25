//! Character-progression messages: XP, exploration, level-up, talent learning and talent wipe.

use std::io::{self, Read};

use crate::wire::{read_f32_le, read_u32_le, read_u64_le, read_u8};

/// `SMSG_LOG_XPGAIN`: an XP award; `victim` is 0 for non-kill XP (vmangos `Misc.cpp:512-522`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XpGain {
    pub victim: u64,
    pub total: u32,
    /// The award before the rested bonus; only a kill carries it, else it equals `total`.
    /// `total - base` is the chat line's "(+N exp Rested bonus)".
    pub base: u32,
    pub kill: bool,
}

/// Read `SMSG_LOG_XPGAIN`: `u64 victim, u32 total, u8 type` (0 kill, 1 other), then for a kill
/// `u32 base, f32 groupBonus`. The group bonus is read and dropped, its chat line not built;
/// vmangos always sends 1.0, meaning none (`Player.cpp:3039`).
pub(super) fn read_xp_gain(r: &mut impl Read) -> io::Result<XpGain> {
    let victim = read_u64_le(r)?;
    let total = read_u32_le(r)?;
    let xp_type = read_u8(r)?;
    let kill = xp_type == 0;
    let mut base = total;
    if kill {
        base = read_u32_le(r)?;
        let _group_bonus = read_f32_le(r)?;
    }
    Ok(XpGain {
        victim,
        total,
        base,
        kill,
    })
}

/// `SMSG_EXPLORATION_EXPERIENCE`: a first visit to an area, sent even for 0 XP
/// (`Player.cpp:6228-6341`). The XP also arrives as a non-kill [`XpGain`]; this names the area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExplorationXp {
    /// The `AreaTable.dbc` row id, not the explore-flag bit.
    pub area_id: u32,
    /// The XP awarded; 0 at max level or for an area without a level.
    pub xp: u32,
}

/// Read `SMSG_EXPLORATION_EXPERIENCE`: `u32 areaId, u32 xp` (vmangos `Misc.cpp:552-556`).
pub(super) fn read_exploration_xp(r: &mut impl Read) -> io::Result<ExplorationXp> {
    let area_id = read_u32_le(r)?;
    let xp = read_u32_le(r)?;
    Ok(ExplorationXp { area_id, xp })
}

/// Body of `CMSG_LEARN_TALENT`: `u32` `Talent.dbc` id, `u32` 0-based rank. Requesting rank k
/// learns up to k (`Player.cpp:20807`), so a click sends the current rank count.
pub fn learn_talent(talent_id: u32, requested_rank: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&talent_id.to_le_bytes());
    body.extend_from_slice(&requested_rank.to_le_bytes());
    body
}

/// Inbound `MSG_TALENT_WIPE_CONFIRM`: a class trainer asks whether to unlearn every talent
/// (vmangos `Player.cpp:8414`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TalentWipeConfirm {
    /// The asking trainer, echoed in the answer and required to be a trainer in range there.
    /// Zero means "you have no talents to reset" (vmangos `SkillHandler.cpp:52`).
    pub trainer: u64,
    /// Cost in copper, climbing with every reset (`Player::GetResetTalentsCost`).
    pub cost: u32,
}

/// Read the inbound `MSG_TALENT_WIPE_CONFIRM`: `u64 trainer, u32 cost` (`Skill.cpp:19-23`).
pub(super) fn read_talent_wipe_confirm(r: &mut impl Read) -> io::Result<TalentWipeConfirm> {
    Ok(TalentWipeConfirm {
        trainer: read_u64_le(r)?,
        cost: read_u32_le(r)?,
    })
}

/// Body of the outbound `MSG_TALENT_WIPE_CONFIRM`, the `CONFIRM_TALENT_WIPE` Accept: the trainer
/// guid alone (vmangos `Skill.cpp:14-17`), as the reference latches it at `0xc4d7a0`.
pub fn talent_wipe_confirm(trainer_guid: u64) -> Vec<u8> {
    trainer_guid.to_le_bytes().to_vec()
}

/// `SMSG_LEVELUP_INFO`: our own level-up, sent only to us with no guid (`Player.cpp:3210-3218`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelUpInfo {
    /// The level just reached.
    pub level: u32,
    /// Hit points gained.
    pub health: u32,
    /// Power gains: mana, rage, focus, energy, happiness. The server fills only mana, which is
    /// the reference chat line's mana argument.
    pub powers: [u32; 5],
    /// Stat gains in `SPELL_STAT0..4` order (str, agi, stam, int, spirit), one chat line per
    /// positive entry.
    pub stats: [u32; 5],
}

/// Read `SMSG_LEVELUP_INFO`: `u32` level, health, 5 powers, 5 stats (`Misc.cpp:524-532`).
pub(super) fn read_level_up_info(r: &mut impl Read) -> io::Result<LevelUpInfo> {
    let level = read_u32_le(r)?;
    let health = read_u32_le(r)?;
    let mut powers = [0u32; 5];
    for p in &mut powers {
        *p = read_u32_le(r)?;
    }
    let mut stats = [0u32; 5];
    for s in &mut stats {
        *s = read_u32_le(r)?;
    }
    Ok(LevelUpInfo {
        level,
        health,
        powers,
        stats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_talent_wipe_pair_round_trips() {
        let trainer: u64 = 0xF130_0000_0000_2A01;
        let mut body = trainer.to_le_bytes().to_vec();
        body.extend_from_slice(&15_000u32.to_le_bytes());
        assert_eq!(
            read_talent_wipe_confirm(&mut &body[..]).unwrap(),
            TalentWipeConfirm {
                trainer,
                cost: 15_000
            }
        );
        assert_eq!(talent_wipe_confirm(trainer), trainer.to_le_bytes().to_vec());
    }

    #[test]
    fn a_zero_trainer_is_a_parseable_refusal() {
        let mut body = 0u64.to_le_bytes().to_vec();
        body.extend_from_slice(&0u32.to_le_bytes());
        let ask = read_talent_wipe_confirm(&mut &body[..]).unwrap();
        assert_eq!(ask.trainer, 0);
    }
}
