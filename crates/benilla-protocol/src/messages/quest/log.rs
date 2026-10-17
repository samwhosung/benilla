//! Quest-log wire (vmangos `Server/Packets/Quest.cpp`): the quest query and response (92/93), log
//! swap, remove and full (403-405), and the `SMSG_QUESTUPDATE_*` progress pushes (406-410).

use std::io;

use crate::wire::{read_cstring, read_f32_le, read_i32_le, read_u32_le, read_u64_le};

/// Objective quads the response always writes, the unused tail zero-filled (`QuestDef.h:34-43`).
pub const QUEST_OBJECTIVES_COUNT: u32 = 4;
/// `{itemId, count}` reward slots the response always writes.
pub const QUEST_REWARDS_COUNT: u32 = 4;
/// `{itemId, count}` reward-choice slots the response always writes.
pub const QUEST_REWARD_CHOICES_COUNT: u32 = 6;

/// One objective quad plus its text, which the packet sends after all the quads. `creature_or_go`
/// stays raw: a creature entry, or `(-id)|0x80000000` for a gameobject (`Quest.cpp:512-516`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestObjective {
    pub creature_or_go: u32,
    pub required_count: u32,
    pub item_id: u32,
    pub item_count: u32,
    pub text: String,
}

/// `QuestFlags` (vmangos `QuestDef.h:145-160`): the `QuestTemplate::flags` word, the only place
/// the client learns a quest's kind; the giver panels never carry it.
pub mod quest_flags {
    /// Not used by 1.12 data.
    pub const STAY_ALIVE: u32 = 0x0000_0001;
    /// Escort: one member accepting sends every other eligible one `SMSG_QUEST_CONFIRM_ACCEPT`.
    pub const PARTY_ACCEPT: u32 = 0x0000_0002;
    /// Not used by 1.12 data.
    pub const EXPLORATION: u32 = 0x0000_0004;
    /// Shareable. The client gates Share Quest on it: the server never tests it on the push, only
    /// on the receiver's accept (`Player::CanShareQuest`, `Player.cpp:13959`).
    pub const SHARABLE: u32 = 0x0000_0008;
    /// Not used by 1.12 data.
    pub const EPIC: u32 = 0x0000_0020;
    /// Not used by 1.12 data.
    pub const RAID: u32 = 0x0000_0040;
    /// Rewards ship only in `SMSG_QUESTGIVER_OFFER_REWARD`; DETAILS and this template zero them.
    pub const HIDDEN_REWARDS: u32 = 0x0000_0200;
    /// Granted and completed in one step; never appears in the client's quest log.
    pub const AUTO_REWARDED: u32 = 0x0000_0400;
}

/// `SMSG_QUEST_QUERY_RESPONSE` (vmangos `Quest.cpp:393-528`, the `>1_9_4` branch): the quest
/// template behind the quest log's detail view. Its arrays are fixed-count with no display ids.
#[derive(Debug, Clone, PartialEq)]
pub struct QuestTemplate {
    pub quest_id: u32,
    pub method: u32,
    pub level: u32,
    pub zone_or_sort: i32,
    pub quest_type: u32,
    pub rep_objective_faction: u32,
    pub rep_objective_value: u32,
    /// The faction whose standing must not pass [`Self::opposite_rep_value`] at turn-in
    /// (`+0x1c`, `+0x20`, the client's `0x4df6e3`); vmangos always sends 0.
    pub opposite_rep_faction: u32,
    pub opposite_rep_value: u32,
    pub next_quest_in_chain: u32,
    pub money: i32,
    pub money_max_level: u32,
    pub reward_spell: u32,
    pub src_item_id: u32,
    pub flags: u32,
    pub rewards: [(u32, u32); QUEST_REWARDS_COUNT as usize],
    pub choices: [(u32, u32); QUEST_REWARD_CHOICES_COUNT as usize],
    pub point_map_id: u32,
    pub point_x: f32,
    pub point_y: f32,
    pub point_opt: u32,
    pub title: String,
    pub objectives_text: String,
    pub details: String,
    pub end_text: String,
    pub objectives: [QuestObjective; QUEST_OBJECTIVES_COUNT as usize],
}

// ── CMSG encoders ────────────────────────────────────────────────────────────────────────────────

/// Body of `CMSG_QUEST_QUERY` (`Quest.cpp:6`): the quest id. Unlike the giver query, no NPC guid.
pub fn quest_query(quest_id: u32) -> Vec<u8> {
    quest_id.to_le_bytes().to_vec()
}

/// Body of `CMSG_QUESTLOG_REMOVE_QUEST` (`Quest.cpp:58`): abandons a log slot. There is no reply;
/// the cleared `PLAYER_QUEST_LOG` fields are the confirmation.
pub fn questlog_remove_quest(slot: u8) -> Vec<u8> {
    vec![slot]
}

/// Body of `CMSG_QUESTLOG_SWAP_QUEST` (`Quest.cpp:52`).
pub fn questlog_swap_quest(slot1: u8, slot2: u8) -> Vec<u8> {
    vec![slot1, slot2]
}

// ── SMSG parsers ─────────────────────────────────────────────────────────────────────────────────

/// Read `SMSG_QUEST_QUERY_RESPONSE`.
pub(in crate::messages) fn read_quest_query_response(r: &mut &[u8]) -> io::Result<QuestTemplate> {
    let quest_id = read_u32_le(r)?;
    let method = read_u32_le(r)?;
    let level = read_u32_le(r)?;
    let zone_or_sort = read_i32_le(r)?;
    let quest_type = read_u32_le(r)?;
    let rep_objective_faction = read_u32_le(r)?;
    let rep_objective_value = read_u32_le(r)?;
    let opposite_rep_faction = read_u32_le(r)?;
    let opposite_rep_value = read_u32_le(r)?;
    let next_quest_in_chain = read_u32_le(r)?;
    let money = read_i32_le(r)?;
    let money_max_level = read_u32_le(r)?;
    let reward_spell = read_u32_le(r)?;
    let src_item_id = read_u32_le(r)?;
    let flags = read_u32_le(r)?;

    // Fixed-count, unlike the giver panels' count-prefixed blocks; HIDDEN_REWARDS only zeroes them.
    let mut rewards = [(0u32, 0u32); QUEST_REWARDS_COUNT as usize];
    for slot in rewards.iter_mut() {
        *slot = (read_u32_le(r)?, read_u32_le(r)?);
    }
    let mut choices = [(0u32, 0u32); QUEST_REWARD_CHOICES_COUNT as usize];
    for slot in choices.iter_mut() {
        *slot = (read_u32_le(r)?, read_u32_le(r)?);
    }

    let point_map_id = read_u32_le(r)?;
    let point_x = read_f32_le(r)?;
    let point_y = read_f32_le(r)?;
    let point_opt = read_u32_le(r)?;

    let title = read_cstring(r)?;
    // Objectives before details: the reverse of the giver panel's `QuestDetails` order.
    let objectives_text = read_cstring(r)?;
    let details = read_cstring(r)?;
    let end_text = read_cstring(r)?;

    let mut raw_objectives = [(0u32, 0u32, 0u32, 0u32); QUEST_OBJECTIVES_COUNT as usize];
    for slot in raw_objectives.iter_mut() {
        *slot = (
            read_u32_le(r)?,
            read_u32_le(r)?,
            read_u32_le(r)?,
            read_u32_le(r)?,
        );
    }
    // The objective texts follow all the quads, not interleaved with them.
    let mut objective_texts = Vec::with_capacity(QUEST_OBJECTIVES_COUNT as usize);
    for _ in 0..QUEST_OBJECTIVES_COUNT {
        objective_texts.push(read_cstring(r)?);
    }
    let objectives: Vec<QuestObjective> = raw_objectives
        .into_iter()
        .zip(objective_texts)
        .map(
            |((creature_or_go, required_count, item_id, item_count), text)| QuestObjective {
                creature_or_go,
                required_count,
                item_id,
                item_count,
                text,
            },
        )
        .collect();
    let objectives: [QuestObjective; QUEST_OBJECTIVES_COUNT as usize] = objectives
        .try_into()
        .expect("read_quest_query_response always reads QUEST_OBJECTIVES_COUNT quads/texts");

    Ok(QuestTemplate {
        quest_id,
        method,
        level,
        zone_or_sort,
        quest_type,
        rep_objective_faction,
        rep_objective_value,
        opposite_rep_faction,
        opposite_rep_value,
        next_quest_in_chain,
        money,
        money_max_level,
        reward_spell,
        src_item_id,
        flags,
        rewards,
        choices,
        point_map_id,
        point_x,
        point_y,
        point_opt,
        title,
        objectives_text,
        details,
        end_text,
        objectives,
    })
}

/// Read `SMSG_QUESTUPDATE_ADD_KILL` (`Quest.cpp:144`): `(questId, entry, count, required, guid)`.
/// A gameobject `entry` is `(-id)|0x80000000` (`Player.cpp:14634`); `count` stays below 64. This
/// is the toast line only: the `PLAYER_QUEST_LOG` counter field is the durable state.
pub(in crate::messages) fn read_quest_update_add_kill(
    r: &mut &[u8],
) -> io::Result<(u32, u32, u32, u32, u64)> {
    Ok((
        read_u32_le(r)?,
        read_u32_le(r)?,
        read_u32_le(r)?,
        read_u32_le(r)?,
        read_u64_le(r)?,
    ))
}

/// Read `SMSG_QUESTUPDATE_ADD_ITEM` (vmangos `Quest.cpp:138`): `u32 itemId, u32 count`.
pub(in crate::messages) fn read_quest_update_add_item(r: &mut &[u8]) -> io::Result<(u32, u32)> {
    Ok((read_u32_le(r)?, read_u32_le(r)?))
}

/// Read `SMSG_QUESTUPDATE_COMPLETE` (`Quest.cpp:91`): the quest id, once every objective is done.
pub(in crate::messages) fn read_quest_update_complete(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}

/// Read `SMSG_QUESTUPDATE_FAILED` (vmangos `Quest.cpp:116`): one `u32` quest id.
pub(in crate::messages) fn read_quest_update_failed(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}

/// Read `SMSG_QUESTUPDATE_FAILEDTIMER` (`Quest.cpp:121`): the id of a timed quest that expired.
pub(in crate::messages) fn read_quest_update_failedtimer(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Fixture builders ─────────────────────────────────────────────────────────────────────────
    fn push_u32(b: &mut Vec<u8>, v: u32) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn push_i32(b: &mut Vec<u8>, v: i32) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn push_u64(b: &mut Vec<u8>, v: u64) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn push_f32(b: &mut Vec<u8>, v: f32) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn push_cstr(b: &mut Vec<u8>, s: &str) {
        b.extend_from_slice(s.as_bytes());
        b.push(0);
    }

    // ── CMSG encode goldens ──────────────────────────────────────────────────────────────────────

    #[test]
    fn cmsg_quest_query_body() {
        assert_eq!(quest_query(1234), vec![0xD2, 0x04, 0x00, 0x00]);
    }

    #[test]
    fn cmsg_questlog_remove_quest_body() {
        assert_eq!(questlog_remove_quest(5), vec![5]);
    }

    #[test]
    fn cmsg_questlog_swap_quest_body() {
        assert_eq!(questlog_swap_quest(3, 7), vec![3, 7]);
    }

    // ── SMSG parse fixtures ──────────────────────────────────────────────────────────────────────

    #[test]
    fn quest_query_response_parses_all_wire_traps() {
        let mut b = Vec::new();
        push_u32(&mut b, 100); // questId
        push_u32(&mut b, 1); // method
        push_u32(&mut b, 12); // level
        push_i32(&mut b, -3); // zoneOrSort
        push_u32(&mut b, 2); // type
        push_u32(&mut b, 169); // repObjectiveFaction
        push_u32(&mut b, 500); // repObjectiveValue
        push_u32(&mut b, 72); // requiredOppositeRepFaction
        push_u32(&mut b, 3000); // requiredOppositeRepValue
        push_u32(&mut b, 777); // nextQuestInChain
        push_i32(&mut b, 5000); // rewOrReqMoney
        push_u32(&mut b, 10); // rewMoneyMaxLevel
        push_u32(&mut b, 4321); // rewSpell
        push_u32(&mut b, 900); // srcItemId
        push_u32(&mut b, 0x10); // questFlags

        // rewards: 4 fixed slots, only 2 non-zero.
        push_u32(&mut b, 3000);
        push_u32(&mut b, 1);
        push_u32(&mut b, 0);
        push_u32(&mut b, 0);
        push_u32(&mut b, 3005);
        push_u32(&mut b, 3);
        push_u32(&mut b, 0);
        push_u32(&mut b, 0);
        // choices: 6 slots, only 1 non-zero.
        push_u32(&mut b, 0);
        push_u32(&mut b, 0);
        push_u32(&mut b, 4000);
        push_u32(&mut b, 2);
        push_u32(&mut b, 0);
        push_u32(&mut b, 0);
        push_u32(&mut b, 0);
        push_u32(&mut b, 0);
        push_u32(&mut b, 0);
        push_u32(&mut b, 0);
        push_u32(&mut b, 0);
        push_u32(&mut b, 0);

        push_u32(&mut b, 1); // pointMapId
        push_f32(&mut b, 123.5); // pointX
        push_f32(&mut b, -45.25); // pointY
        push_u32(&mut b, 2); // pointOpt

        // cstr order: title, objectives, details, endText.
        push_cstr(&mut b, "A Threat Within");
        push_cstr(&mut b, "Slay 10 kobolds.");
        push_cstr(&mut b, "Kill the kobolds threatening Goldshire.");
        push_cstr(&mut b, "You have done well, adventurer.");

        // Objective quads: [creature, gameobject, item, unused].
        let go_encoded = ((-57i32) as u32) | 0x8000_0000;
        push_u32(&mut b, 100); // objective0: creatureOrGO (creature entry)
        push_u32(&mut b, 10); // requiredCount
        push_u32(&mut b, 0);
        push_u32(&mut b, 0);
        push_u32(&mut b, go_encoded); // objective1: creatureOrGO (gameobject, id 57)
        push_u32(&mut b, 1);
        push_u32(&mut b, 0);
        push_u32(&mut b, 0);
        push_u32(&mut b, 0); // objective2: item objective
        push_u32(&mut b, 0);
        push_u32(&mut b, 2000);
        push_u32(&mut b, 5);
        push_u32(&mut b, 0); // objective3: unused slot
        push_u32(&mut b, 0);
        push_u32(&mut b, 0);
        push_u32(&mut b, 0);
        // Objective texts trail the whole packet, after every quad.
        push_cstr(&mut b, "Kill 10 kobolds");
        push_cstr(&mut b, "Destroy the barricade");
        push_cstr(&mut b, "Collect 5 kobold ears");
        push_cstr(&mut b, "");

        let q = read_quest_query_response(&mut b.as_slice()).unwrap();
        assert_eq!(q.quest_id, 100);
        assert_eq!(q.method, 1);
        assert_eq!(q.level, 12);
        assert_eq!(q.zone_or_sort, -3);
        assert_eq!(q.quest_type, 2);
        assert_eq!(q.rep_objective_faction, 169);
        assert_eq!(q.rep_objective_value, 500);
        assert_eq!((q.opposite_rep_faction, q.opposite_rep_value), (72, 3000));
        assert_eq!(q.next_quest_in_chain, 777);
        assert_eq!(q.money, 5000);
        assert_eq!(q.money_max_level, 10);
        assert_eq!(q.reward_spell, 4321);
        assert_eq!(q.src_item_id, 900);
        assert_eq!(q.flags, 0x10);
        assert_eq!(q.rewards, [(3000, 1), (0, 0), (3005, 3), (0, 0)]);
        assert_eq!(
            q.choices,
            [(0, 0), (4000, 2), (0, 0), (0, 0), (0, 0), (0, 0)]
        );
        assert_eq!(q.point_map_id, 1);
        assert_eq!(q.point_x, 123.5);
        assert_eq!(q.point_y, -45.25);
        assert_eq!(q.point_opt, 2);
        assert_eq!(q.title, "A Threat Within");
        assert_eq!(q.objectives_text, "Slay 10 kobolds.");
        assert_eq!(q.details, "Kill the kobolds threatening Goldshire.");
        assert_eq!(q.end_text, "You have done well, adventurer.");
        assert_eq!(q.objectives[0].creature_or_go, 100);
        assert_eq!(q.objectives[0].required_count, 10);
        assert_eq!(q.objectives[0].text, "Kill 10 kobolds");
        assert_eq!(q.objectives[1].creature_or_go, go_encoded);
        assert_eq!(q.objectives[1].required_count, 1);
        assert_eq!(q.objectives[1].text, "Destroy the barricade");
        assert_eq!(q.objectives[2].item_id, 2000);
        assert_eq!(q.objectives[2].item_count, 5);
        assert_eq!(q.objectives[2].text, "Collect 5 kobold ears");
        assert_eq!(q.objectives[3].text, "");
    }

    #[test]
    fn quest_query_response_hidden_rewards_shape_is_blind() {
        // HIDDEN_REWARDS zeroes the rewards, choices and money but keeps the fixed-count shape.
        let mut b = Vec::new();
        push_u32(&mut b, 200); // questId
        push_u32(&mut b, 0); // method
        push_u32(&mut b, 5); // level
        push_i32(&mut b, 10); // zoneOrSort
        push_u32(&mut b, 1); // type
        push_u32(&mut b, 0); // repObjectiveFaction
        push_u32(&mut b, 0); // repObjectiveValue
        push_u32(&mut b, 0); // requiredOppositeRepFaction
        push_u32(&mut b, 0); // requiredOppositeRepValue
        push_u32(&mut b, 0); // nextQuestInChain
        push_i32(&mut b, 0); // money (hidden rewards ⇒ 0)
        push_u32(&mut b, 0); // rewMoneyMaxLevel
        push_u32(&mut b, 99); // rewSpell
        push_u32(&mut b, 0); // srcItemId
        push_u32(&mut b, 0x200); // questFlags: HIDDEN_REWARDS
        for _ in 0..QUEST_REWARDS_COUNT {
            push_u32(&mut b, 0);
            push_u32(&mut b, 0);
        }
        for _ in 0..QUEST_REWARD_CHOICES_COUNT {
            push_u32(&mut b, 0);
            push_u32(&mut b, 0);
        }
        push_u32(&mut b, 0); // pointMapId
        push_f32(&mut b, 0.0);
        push_f32(&mut b, 0.0);
        push_u32(&mut b, 0);
        push_cstr(&mut b, "T");
        push_cstr(&mut b, "O"); // objectives
        push_cstr(&mut b, "D"); // details
        push_cstr(&mut b, "E"); // endText
        for _ in 0..QUEST_OBJECTIVES_COUNT {
            push_u32(&mut b, 0);
            push_u32(&mut b, 0);
            push_u32(&mut b, 0);
            push_u32(&mut b, 0);
        }
        for _ in 0..QUEST_OBJECTIVES_COUNT {
            push_cstr(&mut b, "");
        }

        let q = read_quest_query_response(&mut b.as_slice()).unwrap();
        assert_eq!(q.rewards, [(0, 0); QUEST_REWARDS_COUNT as usize]);
        assert_eq!(q.choices, [(0, 0); QUEST_REWARD_CHOICES_COUNT as usize]);
        assert_eq!(q.money, 0);
        assert_eq!(q.reward_spell, 99);
    }

    #[test]
    fn quest_update_add_kill_reads_guid() {
        let mut b = Vec::new();
        push_u32(&mut b, 100); // questId
        push_u32(&mut b, 200); // entry
        push_u32(&mut b, 3); // count
        push_u32(&mut b, 10); // required
        push_u64(&mut b, 0xF130_0000_0000_0777); // guid
        let (quest_id, entry, count, required, guid) =
            read_quest_update_add_kill(&mut b.as_slice()).unwrap();
        assert_eq!(quest_id, 100);
        assert_eq!(entry, 200);
        assert_eq!(count, 3);
        assert_eq!(required, 10);
        assert_eq!(guid, 0xF130_0000_0000_0777);
    }

    #[test]
    fn quest_update_simple_smsgs_parse() {
        let mut b = Vec::new();
        push_u32(&mut b, 42);
        assert_eq!(read_quest_update_complete(&mut b.as_slice()).unwrap(), 42);
        assert_eq!(read_quest_update_failed(&mut b.as_slice()).unwrap(), 42);
        assert_eq!(
            read_quest_update_failedtimer(&mut b.as_slice()).unwrap(),
            42
        );

        let mut b2 = Vec::new();
        push_u32(&mut b2, 3000);
        push_u32(&mut b2, 5);
        assert_eq!(
            read_quest_update_add_item(&mut b2.as_slice()).unwrap(),
            (3000, 5)
        );
    }
}
