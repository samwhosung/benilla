//! Wire tests for spells, action buttons, melee and the combat log (`src/messages/spells.rs`).

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages;
use benilla_protocol::ServerPacket;
use common::hx;

#[test]
fn spell_and_action_wire() {
    use benilla_protocol::messages::{ActionButton, CastOutcome, ACTION_KIND_ITEM};

    // SMSG_INITIAL_SPELLS (vmangos `Player::SendInitialSpells`): u8 0; u16 n; n x (u16 id, u16 0);
    // u16 m; m x (u16 spell, u16 item, u16 category, u32 spellCdMs, u32 catCdMs).
    let body = hx(concat!(
        "00", "0300", "4e000000", // 78 Heroic Strike
        "cb190000", // 6603 Attack
        "111a0000", // 6673 Battle Shout
        "0100", "4e00", "0000", "2301", "88130000",
        "dc050000" // 78 on 5000ms cd, category 0x123 1500ms
    ));
    match messages::parse_server(messages::opcode::SMSG_INITIAL_SPELLS, &body).unwrap() {
        ServerPacket::InitialSpells {
            spell_ids,
            cooldowns,
        } => {
            assert_eq!(spell_ids, vec![78, 6603, 6673]);
            assert_eq!(cooldowns.len(), 1);
            assert_eq!((cooldowns[0].spell_id, cooldowns[0].category), (78, 0x123));
            assert_eq!(
                (cooldowns[0].spell_cd_ms, cooldowns[0].category_cd_ms),
                (5000, 1500)
            );
        }
        _ => panic!("initial spells"),
    }

    // SMSG_ACTION_BUTTONS: 120 packed u32s, action bits 0-23, kind bits 24-31, 0 = empty.
    let mut body = vec![0u8; 120 * 4];
    body[0..4].copy_from_slice(&0x0000_1A11u32.to_le_bytes()); // slot 0: spell 6673
    body[4..8].copy_from_slice(&0x8000_0075u32.to_le_bytes()); // slot 1: item 117
    body[44..48].copy_from_slice(&0x4000_0003u32.to_le_bytes()); // slot 11: macro 3
    let packet = messages::parse_server(messages::opcode::SMSG_ACTION_BUTTONS, &body).unwrap();
    let buttons = match &packet {
        ServerPacket::ActionButtons { buttons } => buttons.clone(),
        _ => panic!("action buttons"),
    };
    assert_eq!(
        buttons,
        vec![
            ActionButton {
                slot: 0,
                action: 6673,
                kind: 0
            },
            ActionButton {
                slot: 1,
                action: 117,
                kind: ACTION_KIND_ITEM
            },
            ActionButton {
                slot: 11,
                action: 3,
                kind: 0x40
            },
        ]
    );
    match decode(packet).pop().unwrap() {
        SessionEvent::ActionButtons { buttons: b } => assert_eq!(b, buttons),
        _ => panic!("action buttons event"),
    }

    // CMSG_CAST_SPELL targets per vmangos `SpellCastTargets::read`: packed guids after 1.8.4.
    assert_eq!(
        messages::cast_spell(6673, None),
        hx("111a00000000"),
        "self cast: u32 id + u16 mask 0"
    );
    let victim = 0xF130_0000_4500_002Au64; // creature: high 0xF130, entry 69, counter 0x2A
    assert_eq!(
        messages::cast_spell(5176, Some(victim)),
        hx("381400000200c92a4530f1"),
        "unit cast: u32 id + u16 mask 2 + packed guid (mask 0xC9)"
    );

    // Guid 0x4700_0000_0000_0951 has non-zero bytes 0 (0x51), 1 (0x09), 7 (0x47) → mask 0x83.
    let enchant_target = 0x4700_0000_0000_0951u64;
    assert_eq!(
        messages::cast_spell_item(7418, enchant_target),
        hx("fa1c0000100083510947"),
        "item cast: u32 id + u16 mask 0x10 + packed item guid"
    );

    // CMSG_ATTACKSWING: one full 8-byte guid, not packed.
    assert_eq!(messages::attack_swing(victim), hx("2a000045000030f1"));

    // CMSG_SET_ACTION_BUTTON: u8 button + packed u32 action (vmangos `Misc.cpp:87-90`).
    assert_eq!(
        messages::opcode::CMSG_SET_ACTION_BUTTON,
        0x0128,
        "CMSG_SET_ACTION_BUTTON opcode"
    );
    assert_eq!(
        messages::set_action_button(11, 0x8000_0075),
        hx("0b75000080"),
        "button 11 (lua slot 12), item 117 (kind 0x80<<24 | action 0x75)"
    );
    assert_eq!(
        messages::set_action_button(0, 0),
        hx("0000000000"),
        "packed 0 clears the slot"
    );

    // CMSG_SET_ACTIONBAR_TOGGLES: one u8 (reference `0x4e771d`, vmangos `Misc.cpp:150-153`).
    assert_eq!(
        messages::opcode::CMSG_SET_ACTIONBAR_TOGGLES,
        0x02BF,
        "CMSG_SET_ACTIONBAR_TOGGLES opcode"
    );
    assert_eq!(
        messages::set_actionbar_toggles(0x0b),
        hx("0b"),
        "bars 1, 2 and 4 shown (bits 0x01|0x02|0x08)"
    );
    assert_eq!(
        messages::set_actionbar_toggles(0),
        hx("00"),
        "every extra bar hidden — the default, and a real value rather than an empty body"
    );
    assert_eq!(
        messages::set_actionbar_toggles(0x0f),
        hx("0f"),
        "all four — the largest value the reference binding can ever accumulate"
    );

    assert_eq!(
        messages::opcode::CMSG_SETSHEATHED,
        0x01E0,
        "CMSG_SETSHEATHED opcode"
    );
    assert_eq!(messages::set_sheathed(1), hx("01000000"));

    // SMSG_CAST_RESULT: ok = status 0 ends the body; fail = status 2 + u8 reason.
    match messages::parse_server(messages::opcode::SMSG_CAST_RESULT, &hx("111a000000")).unwrap() {
        ServerPacket::CastResult { spell_id, outcome } => {
            assert_eq!((spell_id, outcome), (6673, CastOutcome::Ok));
        }
        _ => panic!("cast ok"),
    }
    let fail =
        messages::parse_server(messages::opcode::SMSG_CAST_RESULT, &hx("111a00000255")).unwrap();
    match decode(fail).pop().unwrap() {
        SessionEvent::CastResult {
            spell_id,
            success,
            reason,
            arg,
        } => assert_eq!(
            (spell_id, success, reason, arg),
            (6673, false, Some(0x55), None),
            "a reason carrying no argument ends the body at the reason byte"
        ),
        _ => panic!("cast fail event"),
    }
    // Argument words are read by body length, not by reason (vmangos `CastResult::AppendBodyTo`).
    // 12 = Starbreeze Village Moonwell (`SpellFocusObject.dbc`); 0x19's subclass mask is dropped.
    let focus = messages::parse_server(
        messages::opcode::SMSG_CAST_RESULT,
        &hx("111a0000025e0c000000"),
    )
    .unwrap();
    match decode(focus).pop().unwrap() {
        SessionEvent::CastResult { reason, arg, .. } => {
            assert_eq!((reason, arg), (Some(0x5e), Some(12)));
        }
        _ => panic!("cast fail event"),
    }
    let equip = messages::parse_server(
        messages::opcode::SMSG_CAST_RESULT,
        &hx("111a0000021902000000f3a50200"),
    )
    .unwrap();
    match decode(equip).pop().unwrap() {
        SessionEvent::CastResult { reason, arg, .. } => {
            assert_eq!(
                (reason, arg),
                (Some(0x19), Some(2)),
                "arg2 read and dropped"
            );
        }
        _ => panic!("cast fail event"),
    }

    // SMSG_ATTACKSTART: two full guids. SMSG_ATTACKSTOP: two packed guids + u32 isDead.
    let body = hx("01000000000000002a000045000030f1");
    match messages::parse_server(messages::opcode::SMSG_ATTACKSTART, &body).unwrap() {
        ServerPacket::AttackStart {
            attacker,
            victim: v,
        } => {
            assert_eq!((attacker, v), (1, victim));
        }
        _ => panic!("attack start"),
    }
    let body = hx(concat!("0101", "c92a4530f1", "00000000"));
    match messages::parse_server(messages::opcode::SMSG_ATTACKSTOP, &body).unwrap() {
        ServerPacket::AttackStop {
            attacker,
            victim: v,
        } => {
            assert_eq!((attacker, v), (1, victim));
        }
        _ => panic!("attack stop"),
    }

    // SMSG_ATTACKERSTATEUPDATE, per vmangos `Unit.cpp:4572-4605`; the attacker guid comes first.
    let mut body = 0x6u32.to_le_bytes().to_vec(); // HitInfo: 0x2 | 0x4 (offhand)
    body.extend_from_slice(&hx("0101")); // attacker packed guid = 1
    body.extend_from_slice(&hx("c92a4530f1")); // victim packed guid
    body.extend_from_slice(&42u32.to_le_bytes()); // TotalDamage
    body.push(2); // SubDamageCount
    body.extend_from_slice(&0u32.to_le_bytes()); // sub 1: school (physical)
    body.extend_from_slice(&42f32.to_le_bytes()); // sub 1: damage f32
    body.extend_from_slice(&42u32.to_le_bytes()); // sub 1: damage u32
    body.extend_from_slice(&5u32.to_le_bytes()); // sub 1: absorb
    body.extend_from_slice(&0u32.to_le_bytes()); // sub 1: resist
    body.extend_from_slice(&3u32.to_le_bytes()); // sub 2: school (frost)
    body.extend_from_slice(&0f32.to_le_bytes()); // sub 2: damage f32
    body.extend_from_slice(&0u32.to_le_bytes()); // sub 2: damage u32
    body.extend_from_slice(&7u32.to_le_bytes()); // sub 2: absorb
    body.extend_from_slice(&0u32.to_le_bytes()); // sub 2: resist
    body.extend_from_slice(&1u32.to_le_bytes()); // VictimState: hit
    body.extend_from_slice(&0u32.to_le_bytes()); // zero
    body.extend_from_slice(&0u32.to_le_bytes()); // spell id
    body.extend_from_slice(&15u32.to_le_bytes()); // blocked
    match messages::parse_server(messages::opcode::SMSG_ATTACKERSTATEUPDATE, &body).unwrap() {
        ServerPacket::AttackerState(s) => {
            assert_eq!(s.attacker, 1);
            assert_eq!(s.victim, victim);
            assert_eq!(s.hit_info, 0x6);
            assert_eq!(s.damage, 42);
            assert_eq!(s.victim_state, 1);
            assert_eq!(
                s.absorb, 12,
                "absorb summed across both sub-damage blocks (5 + 7)"
            );
            assert_eq!(s.blocked, 15);
            assert_eq!(
                s.school, 0,
                "the wording's school is the FIRST block's (physical), not the second's frost"
            );
        }
        _ => panic!("attacker state"),
    }

    // SMSG_AI_REACTION: raw guid + u32 reaction, 2 = HOSTILE (vmangos `Misc.cpp:445-449`).
    let body = hx(concat!("3412000000000000", "02000000"));
    let packet = messages::parse_server(messages::opcode::SMSG_AI_REACTION, &body).unwrap();
    match &packet {
        ServerPacket::AiReaction { unit, reaction } => {
            assert_eq!((*unit, *reaction), (0x1234, 2));
        }
        other => panic!("ai reaction, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::AiReaction { unit, reaction } => assert_eq!((unit, reaction), (0x1234, 2)),
        other => panic!("ai reaction event, got {other:?}"),
    }
}

/// Every swing refusal has an empty body (vmangos `Server/Packets/Combat.cpp`). DEADTARGET (0x148)
/// and CANT_ATTACK (0x149) share the reference's arm `0x625ab8`, so they decode alike.
#[test]
fn attack_swing_refusal_wire() {
    use benilla_protocol::messages::AttackSwingError;

    for (op, want) in [
        (
            messages::opcode::SMSG_ATTACKSWING_NOTINRANGE,
            AttackSwingError::NotInRange,
        ),
        (
            messages::opcode::SMSG_ATTACKSWING_BADFACING,
            AttackSwingError::BadFacing,
        ),
        (
            messages::opcode::SMSG_ATTACKSWING_DEADTARGET,
            AttackSwingError::DeadOrUnattackable,
        ),
        (
            messages::opcode::SMSG_ATTACKSWING_CANT_ATTACK,
            AttackSwingError::DeadOrUnattackable,
        ),
    ] {
        let packet = messages::parse_server(op, &[]).unwrap_or_else(|e| panic!("{op:#x}: {e}"));
        match &packet {
            ServerPacket::AttackSwingError(got) => assert_eq!(*got, want, "opcode {op:#x}"),
            other => panic!("{op:#x} parsed as {}", other.name()),
        }
        match decode(packet).pop().unwrap() {
            SessionEvent::AttackSwingError(got) => assert_eq!(got, want, "opcode {op:#x} event"),
            other => panic!("{op:#x} event, got {other:?}"),
        }
    }

    // SMSG_CANCEL_COMBAT is the fourth arm: another opcode family, same empty body and effect.
    assert!(
        matches!(
            messages::parse_server(messages::opcode::SMSG_CANCEL_COMBAT, &[]).unwrap(),
            ServerPacket::CancelCombat
        ),
        "SMSG_CANCEL_COMBAT is registered (`0x5e3308` -> `0x5e7dd0`), just not by the combat TU"
    );
    assert!(matches!(
        decode(ServerPacket::CancelCombat).pop().unwrap(),
        SessionEvent::CancelCombat
    ));

    // SMSG_FEIGN_DEATH_RESISTED: in the reference only a `DisplayError(421)` (`0x6e9800`).
    assert!(matches!(
        messages::parse_server(messages::opcode::SMSG_FEIGN_DEATH_RESISTED, &[]).unwrap(),
        ServerPacket::FeignDeathResisted
    ));
    assert!(matches!(
        decode(ServerPacket::FeignDeathResisted).pop().unwrap(),
        SessionEvent::FeignDeathResisted
    ));

    // 0x147 SMSG_ATTACKSWING_NOTSTANDING stays unknown: the reference never registers it (default
    // arm `0x625ade`) and vmangos never sends it.
    assert!(
        matches!(
            messages::parse_server(0x0147, &[]).unwrap(),
            ServerPacket::Other { opcode: 0x0147 }
        ),
        "0x147 NOTSTANDING must stay unparsed — the reference never registers it"
    );
}

/// Spell-visual bodies per vmangos `Spell.cpp:4468-4659` and `4780-4789`,
/// `SpellCastTargetsInfo.cpp:180-234`, `Server/Packets/Misc.cpp:548-550` and
/// `Server/Packets/Spell.cpp:54-58`.
#[test]
fn spell_visual_wire_golden() {
    use benilla_protocol::messages::{SpellCastTargets, SpellGo, SpellStart};

    let creature = 0xF130_0000_4500_002Au64; // packs with mask 0xC9

    // SMSG_SPELL_START, a Fireball-style cast at a unit. The ammo block rides only CAST_FLAG_AMMO.
    let body = hx(concat!(
        "0101",       // item_or_caster pguid: mask 1, guid 1
        "c92a4530f1", // caster pguid: the creature guid, mask 0xC9
        "85000000",   // spellId 133
        "0200",       // castFlags 0x2 (CAST_FLAG_UNKNOWN2)
        "b80b0000",   // remaining cast time: 3000ms
        "0200",       // SpellCastTargets mask: TARGET_FLAG_UNIT (0x2)
        "0155",       // unit target pguid: mask 1, guid 0x55
    ));
    let packet = messages::parse_server(messages::opcode::SMSG_SPELL_START, &body).unwrap();
    match &packet {
        ServerPacket::SpellStart(s) => {
            assert_eq!(
                s,
                &SpellStart {
                    item_or_caster: 1,
                    caster: creature,
                    spell_id: 133,
                    cast_flags: 0x2,
                    cast_time_ms: 3000,
                    targets: SpellCastTargets {
                        mask: 0x2,
                        unit_target: Some(0x55),
                        go_target: None,
                        dest: None,
                    },
                    ammo_display_id: None,
                }
            );
        }
        other => panic!("spell start, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::SpellStart {
            caster,
            spell_id,
            cast_flags,
            cast_time_ms,
            target,
            ammo_display_id,
        } => assert_eq!(
            (
                caster,
                spell_id,
                cast_flags,
                cast_time_ms,
                target,
                ammo_display_id,
            ),
            (creature, 133, 0x2, 3000, Some(0x55), None)
        ),
        other => panic!("spell start event, got {other:?}"),
    }

    let body = hx(concat!(
        "0109",     // item_or_caster pguid: mask 1, guid 9 (no cast item: the caster itself)
        "0109",     // caster pguid: same object, guid 9
        "11000000", // spellId 17
        "0200",     // castFlags 0x2
        "00000000", // remaining cast time: 0 (instant)
        "0000",     // SpellCastTargets mask: 0 (SELF), nothing follows
    ));
    let packet = messages::parse_server(messages::opcode::SMSG_SPELL_START, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::SpellStart {
            caster,
            spell_id,
            cast_time_ms,
            target,
            ..
        } => assert_eq!((caster, spell_id, cast_time_ms, target), (9, 17, 0, None)),
        other => panic!("instant spell start event, got {other:?}"),
    }

    // SMSG_SPELL_GO: only a SPELL_MISS_REFLECT miss carries the extra reflectResult byte.
    let body = hx(concat!(
        "0101",             // item_or_caster pguid: guid 1
        "c92a4530f1",       // caster pguid: the creature
        "4b000000",         // spellId 75 (Auto Shot)
        "2001",             // castFlags 0x120 (CAST_FLAG_UNKNOWN9 0x100 | CAST_FLAG_AMMO 0x20)
        "02",               // hit count 2
        "aa00000000000000", // hit guid 0xAA, raw (unpacked)
        "bb00000000000000", // hit guid 0xBB, raw
        "02",               // miss count 2
        "cc00000000000000",
        "02", // miss guid 0xCC, SPELL_MISS_RESIST (2)
        "dd00000000000000",
        "0b",
        "02",       // miss guid 0xDD, SPELL_MISS_REFLECT (11) + reflectResult 2
        "0200",     // SpellCastTargets mask: TARGET_FLAG_UNIT
        "01ee",     // unit target pguid: guid 0xEE
        "6c170000", // ammo displayId 5996
        "12000000", // ammo inventoryType 18 (dropped)
    ));
    let packet = messages::parse_server(messages::opcode::SMSG_SPELL_GO, &body).unwrap();
    match &packet {
        ServerPacket::SpellGo(s) => {
            assert_eq!(
                s,
                &SpellGo {
                    item_or_caster: 1,
                    caster: creature,
                    spell_id: 75,
                    cast_flags: 0x120,
                    hits: vec![0xAA, 0xBB],
                    misses: vec![(0xCC, 2), (0xDD, 11)],
                    targets: SpellCastTargets {
                        mask: 0x2,
                        unit_target: Some(0xEE),
                        go_target: None,
                        dest: None,
                    },
                    ammo_display_id: Some(5996),
                }
            );
        }
        other => panic!("spell go, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::SpellGo {
            caster,
            spell_id,
            cast_flags,
            hits,
            misses,
            target,
            go_target,
            dest,
            ammo_display_id,
            item_caster,
        } => {
            assert_eq!(caster, creature);
            assert_eq!(spell_id, 75);
            assert_eq!(cast_flags, 0x120);
            assert_eq!(hits, vec![0xAA, 0xBB]);
            assert_eq!(misses, vec![(0xCC, 2), (0xDD, 11)]);
            assert_eq!(target, Some(0xEE));
            assert_eq!(go_target, None); // a unit-targeted cast leaves the GO target empty
            assert_eq!(dest, None); // …and the ground point empty (mask 0x2, no 0x40)
            assert_eq!(ammo_display_id, Some(5996));
            // A first guid that differs from the caster marks an item cast.
            assert_eq!(item_caster, Some(1));
        }
        other => panic!("spell go event, got {other:?}"),
    }

    // A ground cast (Blizzard): mask 0x0040 + dest, a real cast's DYNAMICOBJECT_POS.
    let body = hx(concat!(
        "011a",     // item_or_caster pguid: guid 0x1a (== caster: a plain spell, no item)
        "011a",     // caster pguid
        "0a000000", // spellId 10 (Blizzard)
        "0001",     // castFlags 0x100 (CAST_FLAG_UNKNOWN9)
        "00",       // hit count 0
        "00",       // miss count 0
        "4000",     // SpellCastTargets mask: TARGET_FLAG_DEST_LOCATION
        "8f300ac6", // dest x −8844.14 (0xc60a308f)
        "1f352744", // dest y 668.83 (0x4427351f)
        "b89ec342", // dest z 97.81 (0x42c39eb8)
    ));
    let packet = messages::parse_server(messages::opcode::SMSG_SPELL_GO, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::SpellGo {
            caster,
            spell_id,
            hits,
            misses,
            target,
            dest,
            item_caster,
            ..
        } => {
            assert_eq!((caster, spell_id), (0x1a, 10));
            assert!(hits.is_empty() && misses.is_empty());
            assert_eq!(target, None);
            assert_eq!(
                dest,
                Some([
                    f32::from_bits(0xc60a308f),
                    f32::from_bits(0x4427351f),
                    f32::from_bits(0x42c39eb8),
                ])
            );
            assert_eq!(item_caster, None, "same guid twice — not an item cast");
        }
        other => panic!("ground spell go event, got {other:?}"),
    }

    // A GameObject caster (Lightwell 181102, spell 7001): vmangos writes the null `m_casterUnit`
    // as a lone zero mask byte (`Spell.cpp:4525`), so the object rides only slot 1.
    const LIGHTWELL: u64 = 0xF110_02C3_6E00_0123; // GO high guid, entry 181102 << 24, counter 0x123
    let body = hx(concat!(
        "fb23016ec30210f1", // item_or_caster pguid: the GameObject (mask 0xfb: byte 2 is zero)
        "00",               // caster pguid: empty, vmangos writes a null `m_casterUnit`
        "591b0000",         // spellId 7001 (Lightwell Renew)
        "0001",             // castFlags 0x100 (CAST_FLAG_UNKNOWN9)
        "01",               // hit count 1
        "4500000000000000", // hit: the clicking player (raw u64)
        "00",               // miss count 0
        "0200",             // SpellCastTargets mask: TARGET_FLAG_UNIT
        "0145",             // …the same player, packed
    ));
    let packet = messages::parse_server(messages::opcode::SMSG_SPELL_GO, &body).unwrap();
    match &packet {
        ServerPacket::SpellGo(g) => {
            assert_eq!((g.item_or_caster, g.caster), (LIGHTWELL, 0));
        }
        other => panic!("lightwell spell go, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::SpellGo {
            caster,
            spell_id,
            hits,
            target,
            item_caster,
            ..
        } => {
            assert_eq!(
                caster, LIGHTWELL,
                "an empty caster slot falls back to slot 1 — the GameObject IS the caster"
            );
            assert_eq!((spell_id, hits, target), (7001, vec![0x45], Some(0x45)));
            assert_eq!(
                item_caster, None,
                "the object is the caster, not a cast item — the derivation reads the RESOLVED caster"
            );
        }
        other => panic!("lightwell spell go event, got {other:?}"),
    }

    // SMSG_SPELL_FAILED_OTHER: raw (unpacked) guid + spellId (vmangos `Spell::SendInterrupted`).
    let body = hx(concat!("4200000000000000", "85000000"));
    let packet = messages::parse_server(messages::opcode::SMSG_SPELL_FAILED_OTHER, &body).unwrap();
    match &packet {
        ServerPacket::SpellFailedOther { caster, spell_id } => {
            assert_eq!((*caster, *spell_id), (0x42, 133));
        }
        other => panic!("spell failed other, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::SpellFailedOther { caster, spell_id } => {
            assert_eq!((caster, spell_id), (0x42, 133));
        }
        other => panic!("spell failed other event, got {other:?}"),
    }

    // SMSG_SPELL_DELAYED: raw guid + u32 pushback ms (vmangos `Spell::Delayed`).
    let body = hx(concat!("4200000000000000", "f4010000"));
    let packet = messages::parse_server(messages::opcode::SMSG_SPELL_DELAYED, &body).unwrap();
    match &packet {
        ServerPacket::SpellDelayed { caster, delay_ms } => {
            assert_eq!((*caster, *delay_ms), (0x42, 500));
        }
        other => panic!("spell delayed, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::SpellDelayed { caster, delay_ms } => {
            assert_eq!((caster, delay_ms), (0x42, 500));
        }
        other => panic!("spell delayed event, got {other:?}"),
    }

    // SMSG_CANCEL_AUTO_REPEAT: empty body (vmangos `CancelAutoRepeat::AppendBodyTo`).
    let packet = messages::parse_server(messages::opcode::SMSG_CANCEL_AUTO_REPEAT, &[]).unwrap();
    assert!(matches!(packet, ServerPacket::CancelAutoRepeat));
    assert!(matches!(
        decode(packet).pop().unwrap(),
        SessionEvent::CancelAutoRepeat
    ));

    // SMSG_PLAY_SPELL_VISUAL: raw (unpacked) guid + kit id.
    let body = hx(concat!("3412000000000000", "39000000"));
    let packet = messages::parse_server(messages::opcode::SMSG_PLAY_SPELL_VISUAL, &body).unwrap();
    match &packet {
        ServerPacket::PlaySpellVisual { unit, kit_id } => {
            assert_eq!((*unit, *kit_id), (0x1234, 57));
        }
        other => panic!("play spell visual, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::PlaySpellVisual { unit, kit_id } => assert_eq!((unit, kit_id), (0x1234, 57)),
        other => panic!("play spell visual event, got {other:?}"),
    }

    // MSG_CHANNEL_START: u32 spellId + u32 duration, no guid since only the caster gets it
    // (vmangos `Spell.cpp:4963-4966`). 10797 is Starshards.
    let body = hx(concat!("2d2a0000", "70170000"));
    let packet = messages::parse_server(messages::opcode::MSG_CHANNEL_START, &body).unwrap();
    match &packet {
        ServerPacket::ChannelStart {
            spell_id,
            duration_ms,
        } => assert_eq!((*spell_id, *duration_ms), (10797, 6000)),
        other => panic!("channel start, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::ChannelStart {
            spell_id,
            duration_ms,
        } => assert_eq!((spell_id, duration_ms), (10797, 6000)),
        other => panic!("channel start event, got {other:?}"),
    }

    // MSG_CHANNEL_UPDATE: u32 ms left, 0 = over (vmangos `Player.cpp:21141-21146`).
    let body = hx("b80b0000");
    let packet = messages::parse_server(messages::opcode::MSG_CHANNEL_UPDATE, &body).unwrap();
    match &packet {
        ServerPacket::ChannelUpdate { remaining_ms } => assert_eq!(*remaining_ms, 3000),
        other => panic!("channel update, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::ChannelUpdate { remaining_ms } => assert_eq!(remaining_ms, 3000),
        other => panic!("channel update event, got {other:?}"),
    }

    let body = hx(concat!(
        "0101",
        "c92a4530f1",
        "85000000",
        "0200", // truncated before the u32 cast-time
    ));
    assert!(messages::parse_server(messages::opcode::SMSG_SPELL_START, &body).is_err());
    let body = hx(concat!(
        "0101",
        "c92a4530f1",
        "4b000000",
        "2001",
        "02",
        "aa00000000000000", // hit count says 2, only 1 hit guid present
    ));
    assert!(messages::parse_server(messages::opcode::SMSG_SPELL_GO, &body).is_err());
}

/// An open-lock cast names its chest or door as a `TARGET_FLAG_GAMEOBJECT` target, not a unit.
#[test]
fn spell_go_surfaces_the_gameobject_target() {
    let body = hx(concat!(
        "0101",     // item_or_caster pguid: guid 1
        "0107",     // caster pguid: guid 7
        "250d0000", // spellId 3365 ("Opening")
        "0001",     // castFlags 0x100 (CAST_FLAG_UNKNOWN9, no ammo)
        "00",       // hit count 0
        "00",       // miss count 0
        "0008",     // SpellCastTargets mask: TARGET_FLAG_GAMEOBJECT (0x0800)
        "033412",   // GO target pguid: guid 0x1234
    ));
    let packet = messages::parse_server(messages::opcode::SMSG_SPELL_GO, &body).unwrap();
    match &packet {
        ServerPacket::SpellGo(s) => {
            assert_eq!(s.targets.go_target, Some(0x1234));
            assert_eq!(s.targets.unit_target, None);
        }
        other => panic!("spell go, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::SpellGo {
            spell_id,
            go_target,
            target,
            ..
        } => {
            assert_eq!(spell_id, 3365);
            assert_eq!(go_target, Some(0x1234)); // the chest the open-lock cast will pop
            assert_eq!(target, None);
        }
        other => panic!("spell go event, got {other:?}"),
    }
}

/// Combat-log bodies per vmangos `Server/Packets/Spell.cpp:68-86,124-140`, `Unit.cpp:4395-4443`,
/// `Server/Packets/Combat.cpp:73-79` and `Server/Packets/Misc.cpp:512-532`.
#[test]
fn combat_log_wire_golden() {
    use benilla_protocol::messages::{
        DamageShield, EnvironmentalDamageLog, ExplorationXp, LevelUpInfo, PeriodicAuraLog,
        PeriodicTick, SpellDamageLog, SpellLogMiss, XpGain,
    };

    let creature = 0xF130_0000_4500_002Au64; // packs with mask 0xC9

    let body = hx(concat!(
        "c92a4530f1", // target pguid: the creature
        "0101",       // attacker pguid: guid 1
        "85000000",   // spellId 133
        "f4010000",   // damage 500
        "03",         // school 3 (frost)
        "32000000",   // absorbed 50
        "ecffffff",   // resist -20
        "00",         // periodicLog: false (direct, not a DoT tick)
        "00",         // unused
        "0a000000",   // blocked 10
        "02000000",   // hitInfo 0x2 (SPELL_HIT_TYPE_CRIT)
        "00",         // extendedData (always 0, dropped)
    ));
    let packet =
        messages::parse_server(messages::opcode::SMSG_SPELLNONMELEEDAMAGELOG, &body).unwrap();
    match &packet {
        ServerPacket::SpellDamageLog(s) => {
            assert_eq!(
                s,
                &SpellDamageLog {
                    target: creature,
                    attacker: 1,
                    spell_id: 133,
                    damage: 500,
                    school: 3,
                    absorb: 50,
                    resist: -20,
                    periodic: false,
                    blocked: 10,
                    hit_info: 0x2,
                }
            );
        }
        other => panic!("spell damage log, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::SpellDamageLog(s) => assert_eq!(s.hit_info & 0x2, 0x2, "crit bit"),
        other => panic!("spell damage log event, got {other:?}"),
    }

    // SMSG_PERIODICAURALOG: target, caster, spellId, then count x {u32 auraType, payload}.
    let body = hx(concat!(
        "c92a4530f1", // target pguid: the creature
        "0101",       // caster pguid: guid 1
        "ac000000",   // spellId 172 (Corruption)
        "01000000",   // count 1
        "03000000",   // auraType 3 (PERIODIC_DAMAGE)
        "58000000",   // damage 88
        "06000000",   // school 6 (shadow)
        "0c000000",   // absorb 12
        "fbffffff",   // resist -5
    ));
    let packet = messages::parse_server(messages::opcode::SMSG_PERIODICAURALOG, &body).unwrap();
    match &packet {
        ServerPacket::PeriodicAuraLog(l) => {
            assert_eq!(
                l,
                &PeriodicAuraLog {
                    target: creature,
                    caster: 1,
                    spell_id: 172,
                    ticks: vec![PeriodicTick::Damage {
                        amount: 88,
                        school: 6,
                        absorb: 12,
                        resist: -5,
                    }],
                }
            );
        }
        other => panic!("periodic aura log (damage), got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::PeriodicAuraLog(l) => assert_eq!(l.ticks.len(), 1),
        other => panic!("periodic aura log event, got {other:?}"),
    }

    let body = hx(concat!(
        "0101",     // target pguid: guid 1
        "0101",     // caster pguid: guid 1 (self)
        "59000000", // spellId 89 (Renew)
        "01000000", // count 1
        "08000000", // auraType 8 (PERIODIC_HEAL)
        "2d000000", // amount 45
    ));
    let packet = messages::parse_server(messages::opcode::SMSG_PERIODICAURALOG, &body).unwrap();
    match &packet {
        ServerPacket::PeriodicAuraLog(l) => {
            assert_eq!(
                l,
                &PeriodicAuraLog {
                    target: 1,
                    caster: 1,
                    spell_id: 89,
                    ticks: vec![PeriodicTick::Heal { amount: 45 }],
                }
            );
        }
        other => panic!("periodic aura log (heal), got {}", other.name()),
    }

    // An unhandled auraType has no known payload width, so the decode fails rather than desync.
    let body = hx(concat!(
        "0101",     // target
        "0101",     // caster
        "01000000", // spellId 1
        "01000000", // count 1
        "04000000", // auraType 4 (unhandled)
    ));
    assert!(messages::parse_server(messages::opcode::SMSG_PERIODICAURALOG, &body).is_err());

    // SMSG_SPELLDAMAGESHIELD: `victim` bears the shield; `attacker` takes the damage back.
    let body = hx(concat!(
        "1111000000000000", // victim raw guid 0x1111
        "2222000000000000", // attacker raw guid 0x2222
        "21000000",         // damage 33
        "04000000",         // school 4 (nature)
    ));
    let packet = messages::parse_server(messages::opcode::SMSG_SPELLDAMAGESHIELD, &body).unwrap();
    match &packet {
        ServerPacket::DamageShield(s) => {
            assert_eq!(
                s,
                &DamageShield {
                    victim: 0x1111,
                    attacker: 0x2222,
                    damage: 33,
                    school: 4,
                }
            );
        }
        other => panic!("damage shield, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::DamageShield(s) => assert_eq!((s.victim, s.attacker), (0x1111, 0x2222)),
        other => panic!("damage shield event, got {other:?}"),
    }

    // SMSG_ENVIRONMENTALDAMAGELOG: a raw victim guid (vmangos `ObjectGuid.cpp:174`); absorbed and
    // resist are the post-1.6.1 tail (`Server/Packets/Combat.cpp:58-67`).
    let body = hx(concat!(
        "0300000000000000", // victim raw guid 3
        "02",               // damageType 2 (DAMAGE_FALL)
        "17000000",         // damage 23
        "05000000",         // absorbed 5
        "ffffffff",         // resist -1
    ));
    let packet =
        messages::parse_server(messages::opcode::SMSG_ENVIRONMENTALDAMAGELOG, &body).unwrap();
    match &packet {
        ServerPacket::EnvironmentalDamageLog(e) => {
            assert_eq!(
                e,
                &EnvironmentalDamageLog {
                    victim: 3,
                    damage_type: 2,
                    damage: 23,
                    absorb: 5,
                    resist: -1,
                }
            );
        }
        other => panic!("environmental damage log, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::EnvironmentalDamageLog(e) => {
            assert_eq!((e.victim, e.damage_type, e.damage), (3, 2, 23));
        }
        other => panic!("environmental damage log event, got {other:?}"),
    }

    // SMSG_SPELLLOGMISS: count x {raw target guid, u8 missInfo}; useExtended 1 adds 2 x f32 per
    // entry. REFLECT's extra byte rides only SMSG_SPELL_GO's miss list, not this one.
    let body = hx(concat!(
        "85000000",         // spellId 133
        "4200000000000000", // caster raw guid 0x42
        "00",               // useExtended: 0
        "02000000",         // count 2
        "aa00000000000000", // target 0xAA
        "03",               // missInfo 3 (DODGE)
        "bb00000000000000", // target 0xBB
        "0b",               // missInfo 11 (REFLECT)
    ));
    let packet = messages::parse_server(messages::opcode::SMSG_SPELLLOGMISS, &body).unwrap();
    match &packet {
        ServerPacket::SpellLogMiss(s) => {
            assert_eq!(
                s,
                &SpellLogMiss {
                    spell_id: 133,
                    caster: 0x42,
                    misses: vec![(0xAA, 3), (0xBB, 11)],
                }
            );
        }
        other => panic!("spell log miss, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::SpellLogMiss(s) => assert_eq!(s.misses, vec![(0xAA, 3), (0xBB, 11)]),
        other => panic!("spell log miss event, got {other:?}"),
    }

    let mut body = 0xCCu64.to_le_bytes().to_vec(); // victim
    body.extend_from_slice(&250u32.to_le_bytes()); // total
    body.push(0); // xpType: kill
    body.extend_from_slice(&200u32.to_le_bytes()); // base xp (kept: rested bonus = total − base)
    body.extend_from_slice(&1.25f32.to_le_bytes()); // group bonus (dropped)
    let packet = messages::parse_server(messages::opcode::SMSG_LOG_XPGAIN, &body).unwrap();
    match &packet {
        ServerPacket::XpGain(x) => {
            assert_eq!(
                x,
                &XpGain {
                    victim: 0xCC,
                    total: 250,
                    base: 200,
                    kill: true,
                }
            );
        }
        other => panic!("xp gain (kill), got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::XpGain(x) => assert_eq!((x.victim, x.total, x.kill), (0xCC, 250, true)),
        other => panic!("xp gain event, got {other:?}"),
    }

    let mut body = 0u64.to_le_bytes().to_vec(); // victim: 0 (non-kill xp has no victim)
    body.extend_from_slice(&50u32.to_le_bytes()); // total
    body.push(1); // xpType: non-kill, nothing trails
    let packet = messages::parse_server(messages::opcode::SMSG_LOG_XPGAIN, &body).unwrap();
    match &packet {
        ServerPacket::XpGain(x) => {
            assert_eq!(
                x,
                &XpGain {
                    victim: 0,
                    total: 50,
                    base: 50, // non-kill carries no base on the wire; base = total
                    kill: false,
                }
            );
        }
        other => panic!("xp gain (non-kill), got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::XpGain(x) => assert_eq!((x.victim, x.total, x.kill), (0, 50, false)),
        other => panic!("xp gain event, got {other:?}"),
    }

    // SMSG_EXPLORATION_EXPERIENCE: u32 AreaTable.dbc id (40 = Westfall) + u32 xp, sent even when
    // xp is 0 (vmangos `Misc.cpp:552-556`).
    let mut body = 40u32.to_le_bytes().to_vec();
    body.extend_from_slice(&85u32.to_le_bytes());
    assert_eq!(body.len(), 8);
    let packet =
        messages::parse_server(messages::opcode::SMSG_EXPLORATION_EXPERIENCE, &body).unwrap();
    match &packet {
        ServerPacket::ExplorationXp(x) => {
            assert_eq!(
                x,
                &ExplorationXp {
                    area_id: 40,
                    xp: 85,
                }
            );
        }
        other => panic!("exploration xp, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::ExplorationXp(x) => assert_eq!((x.area_id, x.xp), (40, 85)),
        other => panic!("exploration xp event, got {other:?}"),
    }

    // SMSG_LEVELUP_INFO: twelve u32, level, healthGain, powerGains[5] (mana..happiness) and
    // statGains[5] (str..spirit), no guid (vmangos `Misc.cpp:524-532`).
    let mut body = Vec::new();
    for v in [7u32, 22, 15, 0, 0, 0, 0, 1, 0, 1, 2, 1] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    assert_eq!(body.len(), 48);
    let packet = messages::parse_server(messages::opcode::SMSG_LEVELUP_INFO, &body).unwrap();
    match &packet {
        ServerPacket::LevelUp(l) => {
            assert_eq!(
                l,
                &LevelUpInfo {
                    level: 7,
                    health: 22,
                    powers: [15, 0, 0, 0, 0],
                    stats: [1, 0, 1, 2, 1],
                }
            );
        }
        other => panic!("level up, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::LevelUp(l) => assert_eq!((l.level, l.health, l.powers[0]), (7, 22, 15)),
        other => panic!("level up event, got {other:?}"),
    }
}

/// Raw caster guid, u32 spellId, u32 count, raw guids (vmangos `Spell.cpp:4970-4997`); in the
/// reference (`0x6e9820`) the only writer of the beams' chain-target array (`unit+0xd44`).
#[test]
fn spell_chain_targets_wire() {
    use benilla_protocol::messages::SpellChainTargets;

    const CASTER: u64 = 0x0000_01f1_3045_2ac9;

    let body = hx(concat!(
        "c92a4530f1010000", // caster guid, raw u64 (not packed)
        "b1020000",         // spellId 689 (Drain Life)
        "01000000",         // count 1: a u32, not the u8 the GO lists use
        "aa00000000000000", // target guid 0xAA, raw
    ));
    let packet =
        messages::parse_server(messages::opcode::SMSG_SPELL_UPDATE_CHAIN_TARGETS, &body).unwrap();
    match &packet {
        ServerPacket::SpellChainTargets(c) => assert_eq!(
            c,
            &SpellChainTargets {
                caster: CASTER,
                spell_id: 689,
                targets: vec![0xAA],
            }
        ),
        other => panic!("chain targets, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::SpellChainTargets {
            caster,
            spell_id,
            targets,
        } => assert_eq!((caster, spell_id, targets), (CASTER, 689, vec![0xAA])),
        other => panic!("chain targets event, got {other:?}"),
    }

    let body = hx(concat!(
        "0100000000000000",
        "a5010000", // spellId 421 (Chain Lightning)
        "03000000",
        "aa00000000000000",
        "bb00000000000000",
        "cc00000000000000",
    ));
    match messages::parse_server(messages::opcode::SMSG_SPELL_UPDATE_CHAIN_TARGETS, &body).unwrap()
    {
        ServerPacket::SpellChainTargets(c) => {
            assert_eq!(c.targets, vec![0xAA, 0xBB, 0xCC], "wire order is hop order");
        }
        other => panic!("chain targets, got {}", other.name()),
    }

    // An empty list is legal on the wire, though the server only sends the packet with hits.
    let body = hx(concat!("0100000000000000", "a5010000", "00000000"));
    match messages::parse_server(messages::opcode::SMSG_SPELL_UPDATE_CHAIN_TARGETS, &body).unwrap()
    {
        ServerPacket::SpellChainTargets(c) => assert!(c.targets.is_empty()),
        other => panic!("chain targets, got {}", other.name()),
    }

    // Count 2 with one guid following is an error, not a short list.
    let body = hx(concat!(
        "0100000000000000",
        "a5010000",
        "02000000",
        "aa00000000000000",
    ));
    assert!(
        messages::parse_server(messages::opcode::SMSG_SPELL_UPDATE_CHAIN_TARGETS, &body).is_err()
    );
}

/// Eight combat-log opcodes, each parsed against the vmangos writer cited at it.
#[test]
fn kill_outcome_dispel_enchantment_and_execute_logs_parse() {
    use benilla_protocol::messages::{
        DispelFailed, EnchantmentLog, ExecuteLog, PartyKillLog, SpellDispelLog, SpellInstaKillLog,
        SpellLogExecute, SpellOutcomeLog,
    };

    // SMSG_PARTYKILLLOG: raw killer + raw victim guid (vmangos `Server/Packets/Combat.cpp:52-56`).
    let body = hx(concat!(
        "0700000000000000", // killer 7
        "2a00000000000000", // victim 42
    ));
    match messages::parse_server(messages::opcode::SMSG_PARTYKILLLOG, &body).unwrap() {
        ServerPacket::PartyKillLog(p) => assert_eq!(
            p,
            PartyKillLog {
                killer: 7,
                victim: 42
            }
        ),
        other => panic!("party kill log, got {}", other.name()),
    }

    // SMSG_SPELLINSTAKILLLOG: raw victim guid, u32 spellId (vmangos `SpellEffects.cpp:274-279`).
    let body = hx(concat!("0900000000000000", "0d270000"));
    match messages::parse_server(messages::opcode::SMSG_SPELLINSTAKILLLOG, &body).unwrap() {
        ServerPacket::SpellInstaKillLog(p) => assert_eq!(
            p,
            SpellInstaKillLog {
                victim: 9,
                spell_id: 9997
            }
        ),
        other => panic!("instakill log, got {}", other.name()),
    }

    // One body, two opcodes (vmangos `Server/Packets/Spell.cpp:88-102`).
    let body = hx(concat!(
        "0100000000000000", // caster 1
        "0200000000000000", // target 2
        "39300000",         // spellId 12345
        "01",               // logFormat 1 (the reference's "is periodic" flag)
    ));
    let expect = SpellOutcomeLog {
        caster: 1,
        target: 2,
        spell_id: 12345,
        log_format: 1,
    };
    match messages::parse_server(messages::opcode::SMSG_PROCRESIST, &body).unwrap() {
        ServerPacket::ProcResist(p) => assert_eq!(p, expect),
        other => panic!("proc resist, got {}", other.name()),
    }
    match messages::parse_server(messages::opcode::SMSG_SPELLORDAMAGE_IMMUNE, &body).unwrap() {
        ServerPacket::SpellOrDamageImmune(p) => assert_eq!(p, expect),
        other => panic!("spell-or-damage immune, got {}", other.name()),
    }

    // SMSG_SPELLDISPELLOG, per vmangos `SpellEffects.cpp:2524-2539` (its 1.12.1 branch).
    let body = hx(concat!(
        "0105",     // packed victim: mask 0x01, byte 0x05 -> 5
        "0206",     // packed caster: mask 0x02, byte 0x06 -> 0x600
        "02000000", // count 2
        "0a000000", // 10
        "14000000", // 20
    ));
    match messages::parse_server(messages::opcode::SMSG_SPELLDISPELLOG, &body).unwrap() {
        ServerPacket::SpellDispelLog(p) => assert_eq!(
            p,
            SpellDispelLog {
                victim: 5,
                caster: 0x600,
                spell_ids: vec![10, 20],
            }
        ),
        other => panic!("spell dispel log, got {}", other.name()),
    }

    // SMSG_DISPEL_FAILED: raw caster, raw victim, then spell ids to the end of the body, with no
    // count (vmangos `SpellEffects.cpp:2549-2555`).
    let body = hx(concat!(
        "0300000000000000",
        "0400000000000000",
        "63000000",
        "64000000",
    ));
    match messages::parse_server(messages::opcode::SMSG_DISPEL_FAILED, &body).unwrap() {
        ServerPacket::DispelFailed(p) => assert_eq!(
            p,
            DispelFailed {
                caster: 3,
                victim: 4,
                spell_ids: vec![99, 100],
            }
        ),
        other => panic!("dispel failed, got {}", other.name()),
    }
    let body = hx(concat!("0300000000000000", "0400000000000000"));
    match messages::parse_server(messages::opcode::SMSG_DISPEL_FAILED, &body).unwrap() {
        ServerPacket::DispelFailed(p) => assert!(p.spell_ids.is_empty()),
        other => panic!("dispel failed, got {}", other.name()),
    }

    // SMSG_ENCHANTMENTLOG, per vmangos `Server/Packets/Item.cpp:235-242`.
    let body = hx(concat!(
        "0000000000000000", // caster 0: the enchant faded
        "0b00000000000000", // owner 11
        "d2040000",         // itemEntry 1234
        "39050000",         // spellId 1337
        "00",               // showAffiliation false
    ));
    match messages::parse_server(messages::opcode::SMSG_ENCHANTMENTLOG, &body).unwrap() {
        ServerPacket::EnchantmentLog(p) => assert_eq!(
            p,
            EnchantmentLog {
                caster: 0,
                owner: 11,
                item_entry: 1234,
                spell_id: 1337,
                show_affiliation: false,
            }
        ),
        other => panic!("enchantment log, got {}", other.name()),
    }

    // SMSG_SPELLLOGEXECUTE (vmangos `Spell.cpp:4662-4778`): row shape varies by effect.
    let body = hx(concat!(
        "0107",     // packed caster 7
        "e8030000", // spellId 1000
        "03000000", // 3 groups
        // group 1: effect 8 POWER_DRAIN, one row
        "08000000",
        "01000000",
        "1400000000000000", // target 20
        "2c010000",         // amount 300
        "00000000",         // power 0 (mana)
        "0000803f",         // multiplier 1.0
        // group 2: effect 24 CREATE_ITEM, one row of just an item entry
        "18000000",
        "01000000",
        "b80b0000", // itemEntry 3000
        // group 3: effect 102 DISMISS_PET, one row of just a guid
        "66000000",
        "01000000",
        "1e00000000000000", // target 30
    ));
    match messages::parse_server(messages::opcode::SMSG_SPELLLOGEXECUTE, &body).unwrap() {
        ServerPacket::SpellLogExecute(p) => assert_eq!(
            p,
            SpellLogExecute {
                caster: 7,
                spell_id: 1000,
                effects: vec![
                    (
                        8,
                        vec![ExecuteLog::PowerDrain {
                            target: 20,
                            amount: 300,
                            power: 0,
                            multiplier: 1.0,
                        }]
                    ),
                    (24, vec![ExecuteLog::CreateItem { item_entry: 3000 }]),
                    (102, vec![ExecuteLog::Target { target: 30 }]),
                ],
            }
        ),
        other => panic!("spell log execute, got {}", other.name()),
    }

    // An effect vmangos has no case for has no known row width, so the body errors; vmangos never
    // sends one, since its switch returns before `SendMessageToSet`.
    let body = hx(concat!(
        "0107", "e8030000", "01000000", "77000000", "01000000"
    ));
    assert!(messages::parse_server(messages::opcode::SMSG_SPELLLOGEXECUTE, &body).is_err());
}

/// The flat and pct spell-modifier opcodes share one body; only the opcode sets `flat`.
#[test]
fn spell_modifier_wire() {
    // u8 mask_bit 35, u8 op 14 (SPELLMOD_COST), i32 value -30. Bit 35, Cure Poison 526's only
    // bit, sits past the low dword that a 32-bit mask would keep.
    let body = hx("230ee2ffffff");
    for (wire_op, is_flat) in [
        (messages::opcode::SMSG_SET_FLAT_SPELL_MODIFIER, true),
        (messages::opcode::SMSG_SET_PCT_SPELL_MODIFIER, false),
    ] {
        let packet = messages::parse_server(wire_op, &body).unwrap();
        assert_eq!(
            packet.name(),
            if is_flat {
                "SMSG_SET_FLAT_SPELL_MODIFIER"
            } else {
                "SMSG_SET_PCT_SPELL_MODIFIER"
            }
        );
        match &packet {
            ServerPacket::SpellModifier {
                flat,
                mask_bit,
                op,
                value,
            } => assert_eq!((*flat, *mask_bit, *op, *value), (is_flat, 35, 14, -30)),
            other => panic!("spell modifier, got {}", other.name()),
        }
        match decode(packet).pop().unwrap() {
            SessionEvent::SpellModifier {
                flat,
                mask_bit,
                op,
                value,
            } => assert_eq!((flat, mask_bit, op, value), (is_flat, 35, 14, -30)),
            _ => panic!("spell modifier event"),
        }
    }

    assert!(messages::parse_server(
        messages::opcode::SMSG_SET_FLAT_SPELL_MODIFIER,
        &hx("230ee2ffff")
    )
    .is_err());
}
