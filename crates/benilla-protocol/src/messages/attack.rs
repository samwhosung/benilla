//! Melee auto-attack messages: the attack start and stop edges, the per-swing report, and the
//! creature-aggro notice, which vmangos sends from `Unit::Attack`.

use std::io::{self, Read};

use crate::wire::{read_f32_le, read_packed_guid, read_u32_le, read_u64_le, read_u8};

/// `SMSG_ATTACKSTART` (vmangos `AttackStart::AppendBodyTo`): two full `u64` guids.
pub(super) fn read_attack_start(r: &mut impl Read) -> io::Result<(u64, u64)> {
    Ok((read_u64_le(r)?, read_u64_le(r)?))
}

/// `SMSG_ATTACKERSTATEUPDATE`: one melee swing (`Unit.cpp:4572-4605`), sent once per weapon-timer
/// cycle per hand; the reference plays one swing animation per packet. `hit_info` bit `0x4` marks
/// an offhand swing, bit `0x10000` suppresses the swing animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttackerState {
    pub attacker: u64,
    pub victim: u64,
    pub hit_info: u32,
    /// `TotalDamage`: the whole swing, before the sub-damage split.
    pub damage: u32,
    /// `TargetState` (vmangos `VictimState`): 1 hit, 2 dodge, 3 parry, 4 interrupt, 5 blocks, …
    pub victim_state: u32,
    /// The sub-damage blocks' `absorb`, summed (vmangos writes one block).
    pub absorb: u32,
    /// The sub-damage blocks' `resist`, summed: the partial-resist amount.
    pub resist: i32,
    /// `BlockedAmount`, the trailing blocked-damage word.
    pub blocked: u32,
    /// `meleeSpellId` (`rec+0xac`), nonzero for an on-next-swing ability such as Heroic Strike; the
    /// reference only ever compares it to zero (`0x625e40`, `0x6246ca`).
    pub melee_spell_id: u32,
    /// The first sub-damage block's school (0 physical to 6 arcane, `GetFirstSchoolInMask`), 0
    /// with no blocks; a non-physical swing takes the combat log's `…SCHOOL` wording.
    pub school: u8,
}

impl AttackerState {
    /// `0x625e40`: whether the swing gets a combat-log line (`0x629b10`) and a floating number
    /// (`0x6243e0`). A dodged or parried Heroic Strike gets neither, yet still animates and sounds.
    #[must_use]
    pub fn displayed(&self) -> bool {
        self.melee_spell_id == 0 || self.victim_state == 1
    }
}

/// `SMSG_ATTACKERSTATEUPDATE`: `HitInfo`, attacker then victim packed guid, `TotalDamage`, a `u8`
/// count of `{school u32, damage f32, damage u32, absorb u32, resist i32}`, `TargetState`,
/// `attackerState`, `meleeSpellId`, `BlockedAmount`.
pub(super) fn read_attacker_state(r: &mut impl Read) -> io::Result<AttackerState> {
    let hit_info = read_u32_le(r)?;
    let attacker = read_packed_guid(r)?;
    let victim = read_packed_guid(r)?;
    let damage = read_u32_le(r)?;
    let subs = read_u8(r)?;
    let mut absorb = 0u32;
    let mut resist = 0i32;
    let mut school = 0u8;
    for i in 0..subs {
        // The school comes from the first block; later blocks split off other schools.
        let block_school = read_u32_le(r)?;
        if i == 0 {
            school = u8::try_from(block_school).unwrap_or(0);
        }
        let _damage_f = read_f32_le(r)?;
        let _damage = read_u32_le(r)?;
        absorb += read_u32_le(r)?;
        resist += read_u32_le(r)? as i32;
    }
    let victim_state = read_u32_le(r)?;
    // The tail from `TargetState` (`Unit.cpp:4601`) lands at `rec+0xa4`..`+0xb0`
    // (`0x625c60`); the reference never reads `attackerState`.
    let _attacker_state = read_u32_le(r)?;
    let melee_spell_id = read_u32_le(r)?;
    let blocked = read_u32_le(r)?;
    // The reference handler (`0x625e35`) turns a swing with no damage and a block into
    // `VICTIMSTATE_BLOCKS` (5) before any consumer reads it; vmangos already sends it that way.
    let victim_state = if damage == 0 && blocked != 0 {
        5
    } else {
        victim_state
    };
    Ok(AttackerState {
        melee_spell_id,
        attacker,
        victim,
        hit_info,
        damage,
        victim_state,
        absorb,
        resist,
        blocked,
        school,
    })
}

/// The server's refusal of `CMSG_ATTACKSWING`; every body is empty. Four opcodes, three variants:
/// the reference (`0x6255b0`) handles `0x148` and `0x149` in one shared arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackSwingError {
    /// `SMSG_ATTACKSWING_NOTINRANGE` (`0x145`): the reference latches code 1 (`0x625a8a`).
    NotInRange,
    /// `SMSG_ATTACKSWING_BADFACING` (`0x146`): the reference latches code 2 (`0x625aa1`).
    BadFacing,
    /// `SMSG_ATTACKSWING_DEADTARGET` (`0x148`) or `SMSG_ATTACKSWING_CANT_ATTACK` (`0x149`): no
    /// message, the reference only stops the attack (`0x625ab8`).
    DeadOrUnattackable,
}

/// `SMSG_ATTACKSTOP` (`AttackStop::AppendBodyTo`): two packed guids and a `u32` victim-is-dead
/// word, dropped because death arrives through the descriptor.
pub(super) fn read_attack_stop(r: &mut impl Read) -> io::Result<(u64, u64)> {
    let attacker = read_packed_guid(r)?;
    let victim = read_packed_guid(r)?;
    let _is_dead = read_u32_le(r)?;
    Ok((attacker, victim))
}

/// `SMSG_AI_REACTION` (`Server/Packets/Misc.cpp:445-449`): a raw `u64` guid and a `u32` reaction,
/// 2 (hostile) when a creature starts a melee attack, 0 (alert) on stealth detection; 1 and 4 are
/// never sent.
pub(super) fn read_ai_reaction(r: &mut impl Read) -> io::Result<(u64, u32)> {
    let unit = read_u64_le(r)?;
    let reaction = read_u32_le(r)?;
    Ok((unit, reaction))
}

/// `CMSG_ATTACKSWING`: the full `u64` victim guid, answered by `SMSG_ATTACKSTART` or an
/// [`AttackSwingError`]. `CMSG_ATTACKSTOP` has an empty body.
pub fn attack_swing(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::read_attacker_state;

    /// One swing on the wire, as `SMSG_ATTACKERSTATEUPDATE` lays it out.
    fn packet(damage: u32, victim_state: u32, blocked: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0u32.to_le_bytes()); // HitInfo
        b.extend_from_slice(&[0x01, 0x11]); // attacker PackGUID
        b.extend_from_slice(&[0x01, 0x22]); // victim PackGUID
        b.extend_from_slice(&damage.to_le_bytes()); // TotalDamage
        b.push(1); // one sub-damage block
        b.extend_from_slice(&0u32.to_le_bytes()); // school
        b.extend_from_slice(&(damage as f32).to_le_bytes());
        b.extend_from_slice(&damage.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes()); // absorb
        b.extend_from_slice(&0u32.to_le_bytes()); // resist
        b.extend_from_slice(&victim_state.to_le_bytes()); // TargetState
        b.extend_from_slice(&0u32.to_le_bytes()); // attackerState
        b.extend_from_slice(&0u32.to_le_bytes()); // meleeSpellId
        b.extend_from_slice(&blocked.to_le_bytes()); // BlockedAmount
        b
    }

    #[test]
    fn a_swing_from_a_spell_that_did_not_land_is_not_displayed() {
        let with_id = |state: u32| {
            let mut b = packet(0, state, 0);
            let at = b.len() - 8; // the meleeSpellId slot, before BlockedAmount
            b[at..at + 4].copy_from_slice(&78u32.to_le_bytes()); // Heroic Strike
            read_attacker_state(&mut b.as_slice()).expect("parse")
        };
        // Every state but a hit (1) is suppressed.
        for state in [0, 2, 3, 4] {
            assert!(
                !with_id(state).displayed(),
                "spell swing, victimState {state}"
            );
        }
        assert!(
            with_id(1).displayed(),
            "a landing Heroic Strike still shows"
        );
        for state in [0, 1, 2, 3, 4] {
            let s = read_attacker_state(&mut packet(0, state, 0).as_slice()).expect("parse");
            assert!(s.displayed(), "plain swing, victimState {state}");
        }
    }

    #[test]
    fn a_fully_blocked_swing_is_normalized_to_victimstate_blocks() {
        let s = read_attacker_state(&mut packet(0, 1, 90).as_slice()).expect("parse");
        assert_eq!(s.victim_state, 5, "no damage through, something blocked");

        // A partial block lands damage, so the wire's own state stands.
        let s = read_attacker_state(&mut packet(50, 1, 40).as_slice()).expect("parse");
        assert_eq!(s.victim_state, 1);

        // A dodge blocks nothing and keeps its state, damage or not.
        let s = read_attacker_state(&mut packet(0, 2, 0).as_slice()).expect("parse");
        assert_eq!(s.victim_state, 2);
    }
}
