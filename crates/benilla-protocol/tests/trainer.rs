//! Trainer wire: the list and buy send verbs, the service list, buy results and spell learning.

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{self, train_fail, trainer_spell_state, TrainerSpell};
use benilla_protocol::ServerPacket;

fn hx(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn trainer_send_bodies_golden() {
    // CMSG_TRAINER_LIST: one full trainer guid (vmangos `Npc.cpp` `TrainerList::Read`).
    assert_eq!(
        messages::trainer_list(0x1234_5678_9abc_def0),
        hx("f0debc9a78563412"),
        "CMSG_TRAINER_LIST body"
    );

    // CMSG_TRAINER_BUY_SPELL: u64 trainerGuid + u32 spellId (vmangos `TrainerBuySpell::Read`).
    assert_eq!(
        messages::trainer_buy_spell(0x1234_5678_9abc_def0, 78),
        hx(concat!("f0debc9a78563412", "4e000000")),
        "CMSG_TRAINER_BUY_SPELL body"
    );
}

/// Appends one 38-byte service record in wire order (vmangos `NPCHandler.cpp:97-139`).
fn push_service(
    body: &mut Vec<u8>,
    spell: u32,
    state: u8,
    cost: u32,
    can_learn: u32,
    is_prof: u32,
    req_level: u8,
    req_skill: u32,
    req_skill_value: u32,
    req_spells: [u32; 3],
) {
    body.extend_from_slice(&spell.to_le_bytes());
    body.push(state);
    body.extend_from_slice(&cost.to_le_bytes());
    body.extend_from_slice(&can_learn.to_le_bytes());
    body.extend_from_slice(&is_prof.to_le_bytes());
    body.push(req_level);
    body.extend_from_slice(&req_skill.to_le_bytes());
    body.extend_from_slice(&req_skill_value.to_le_bytes());
    for s in req_spells {
        body.extend_from_slice(&s.to_le_bytes());
    }
}

#[test]
fn trainer_list_wire() {
    // SMSG_TRAINER_LIST, per vmangos `SendTrainerList` (`NPCHandler.cpp:141-241`).
    let mut body = 0xCCu64.to_le_bytes().to_vec();
    body.extend_from_slice(&0u32.to_le_bytes()); // trainerType 0 = class
    body.extend_from_slice(&2u32.to_le_bytes()); // count
    push_service(&mut body, 78, 0, 100, 0, 0, 10, 0, 0, [0, 0, 0]);
    push_service(&mut body, 2018, 2, 1000, 1, 1, 5, 164, 1, [12345, 0, 0]);
    body.extend_from_slice(b"I can teach you.\0");

    match messages::parse_server(messages::opcode::SMSG_TRAINER_LIST, &body).unwrap() {
        ServerPacket::TrainerList {
            trainer,
            trainer_type,
            services,
            title,
        } => {
            assert_eq!((trainer, trainer_type), (0xCC, 0));
            assert_eq!(title, "I can teach you.");
            assert_eq!(
                services,
                vec![
                    TrainerSpell {
                        spell: 78,
                        state: trainer_spell_state::GREEN,
                        cost: 100,
                        can_learn_primary_prof: false,
                        is_primary_prof_first_rank: false,
                        req_level: 10,
                        req_skill: 0,
                        req_skill_value: 0,
                        req_spells: [0, 0, 0],
                    },
                    TrainerSpell {
                        spell: 2018,
                        state: trainer_spell_state::GRAY,
                        cost: 1000,
                        can_learn_primary_prof: true,
                        is_primary_prof_first_rank: true,
                        req_level: 5,
                        req_skill: 164,
                        req_skill_value: 1,
                        req_spells: [12345, 0, 0],
                    },
                ]
            );
        }
        other => panic!("trainer list, got {}", other.name()),
    }

    let mut empty = 0xDDu64.to_le_bytes().to_vec();
    empty.extend_from_slice(&2u32.to_le_bytes()); // trainerType 2 = tradeskill
    empty.extend_from_slice(&0u32.to_le_bytes()); // count
    empty.extend_from_slice(b"Nothing for you yet.\0");
    match messages::parse_server(messages::opcode::SMSG_TRAINER_LIST, &empty).unwrap() {
        ServerPacket::TrainerList {
            trainer,
            trainer_type,
            services,
            title,
        } => {
            assert_eq!((trainer, trainer_type), (0xDD, 2));
            assert!(services.is_empty());
            assert_eq!(title, "Nothing for you yet.");
        }
        other => panic!("empty trainer list, got {}", other.name()),
    }

    let packet = messages::parse_server(messages::opcode::SMSG_TRAINER_LIST, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::TrainerList {
            trainer,
            trainer_type,
            services,
            greeting,
        } => {
            assert_eq!((trainer, trainer_type, services.len()), (0xCC, 0, 2));
            assert_eq!(greeting, "I can teach you.");
        }
        other => panic!("trainer list event, got {other:?}"),
    }
}

#[test]
fn trainer_buy_result_wire() {
    // SMSG_TRAINER_BUY_SUCCEEDED (vmangos `TrainerBuySucceeded`): u64 trainerGuid, u32 spellId.
    let mut ok = 0xCCu64.to_le_bytes().to_vec();
    ok.extend_from_slice(&78u32.to_le_bytes());
    match messages::parse_server(messages::opcode::SMSG_TRAINER_BUY_SUCCEEDED, &ok).unwrap() {
        ServerPacket::TrainerBuySucceeded { trainer, spell_id } => {
            assert_eq!((trainer, spell_id), (0xCC, 78));
        }
        other => panic!("trainer buy succeeded, got {}", other.name()),
    }

    // SMSG_TRAINER_BUY_FAILED (vmangos `TrainerBuyFailed`): u64 guid, u32 serviceId, u32 error.
    let mut failed = 0xCCu64.to_le_bytes().to_vec();
    failed.extend_from_slice(&78u32.to_le_bytes());
    failed.extend_from_slice(&train_fail::NOT_ENOUGH_MONEY.to_le_bytes());
    match messages::parse_server(messages::opcode::SMSG_TRAINER_BUY_FAILED, &failed).unwrap() {
        ServerPacket::TrainerBuyFailed {
            trainer,
            spell_id,
            error,
        } => {
            assert_eq!(
                (trainer, spell_id, error),
                (0xCC, 78, train_fail::NOT_ENOUGH_MONEY)
            );
        }
        other => panic!("trainer buy failed, got {}", other.name()),
    }

    // vmangos values (`Player.h:119-122`, `SharedDefines.h:1120-1122`).
    assert_eq!(
        (
            trainer_spell_state::GREEN,
            trainer_spell_state::RED,
            trainer_spell_state::GRAY,
        ),
        (0, 1, 2)
    );
    assert_eq!(
        (
            train_fail::UNAVAILABLE,
            train_fail::NOT_ENOUGH_MONEY,
            train_fail::NOT_ENOUGH_SKILL,
        ),
        (0, 1, 2)
    );
}

#[test]
fn learned_and_superceded_spell_wire() {
    // SMSG_LEARNED_SPELL: u16 spellId + u16 actionBarSlot, the slot unused by the client and
    // dropped (vmangos `Packets/Spell.cpp:175-179`).
    match messages::parse_server(messages::opcode::SMSG_LEARNED_SPELL, &hx("cb19aaaa")).unwrap() {
        ServerPacket::LearnedSpell { spell_id } => assert_eq!(spell_id, 6603),
        other => panic!("learned spell, got {}", other.name()),
    }
    let packet =
        messages::parse_server(messages::opcode::SMSG_LEARNED_SPELL, &hx("cb19aaaa")).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::SpellLearned { spell_id } => assert_eq!(spell_id, 6603u32),
        other => panic!("spell learned event, got {other:?}"),
    }

    // SMSG_SUPERCEDED_SPELL: u16 oldSpellId + u16 newSpellId (vmangos `Packets/Spell.cpp:169-173`).
    match messages::parse_server(messages::opcode::SMSG_SUPERCEDED_SPELL, &hx("cb19cc19")).unwrap()
    {
        ServerPacket::SupercededSpell {
            old_spell_id,
            new_spell_id,
        } => assert_eq!((old_spell_id, new_spell_id), (6603, 6604)),
        other => panic!("superceded spell, got {}", other.name()),
    }
    let packet =
        messages::parse_server(messages::opcode::SMSG_SUPERCEDED_SPELL, &hx("cb19cc19")).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::SpellSuperceded {
            old_spell_id,
            new_spell_id,
        } => assert_eq!((old_spell_id, new_spell_id), (6603u32, 6604u32)),
        other => panic!("spell superceded event, got {other:?}"),
    }
}
