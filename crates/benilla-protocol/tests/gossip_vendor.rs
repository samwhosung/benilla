//! The gossip and vendor wire: hello, option select, NPC text and the greeting draw, and the vendor
//! inventory, buy and sell messages.

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages;
use benilla_protocol::ServerPacket;

fn hx(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn gossip_bodies_golden() {
    // CMSG_GOSSIP_HELLO (vmangos Npc.cpp:3): a full guid.
    assert_eq!(
        messages::gossip_hello(0x1234_5678_9abc_def0),
        hx("f0debc9a78563412"),
        "CMSG_GOSSIP_HELLO body"
    );

    // CMSG_NPC_TEXT_QUERY (vmangos Npc.cpp:8-12): u32 textID, u64 guid.
    assert_eq!(
        messages::npc_text_query(55, 0x1234_5678_9abc_def0),
        hx(concat!("37000000", "f0debc9a78563412")),
        "CMSG_NPC_TEXT_QUERY body"
    );

    // CMSG_GOSSIP_SELECT_OPTION (vmangos Npc.cpp:78-86): guid, gossipListId, then a code cstring
    // only for a coded option; the server reads it only when bytes remain.
    assert_eq!(
        messages::gossip_select_option(0x1234_5678_9abc_def0, 3, None),
        hx(concat!("f0debc9a78563412", "03000000")),
        "CMSG_GOSSIP_SELECT_OPTION body, no code"
    );
    let mut with_code = hx(concat!("f0debc9a78563412", "03000000"));
    with_code.extend_from_slice(b"1234\0");
    assert_eq!(
        messages::gossip_select_option(0x1234_5678_9abc_def0, 3, Some(b"1234")),
        with_code,
        "CMSG_GOSSIP_SELECT_OPTION body, coded option with a real code"
    );
    // The code goes as a C string, up to its first NUL (`0x418430`: `strlen + 1`).
    let mut cut = hx(concat!("f0debc9a78563412", "03000000"));
    cut.extend_from_slice(b"ab\0");
    assert_eq!(
        messages::gossip_select_option(0x1234_5678_9abc_def0, 3, Some(b"ab\0cd")),
        cut,
        "CMSG_GOSSIP_SELECT_OPTION body, a code cut at its NUL"
    );
}

/// `0x4e271d`-`0x4e2722` reads a count of 16 or more as 15, reads 15 options and takes the quest
/// count from the bytes after them, here the 16th option's index, 0; and the coded byte is tested
/// `& 1` (`0x4e2365`, `0x4e23d4`), so 2 is not coded.
#[test]
fn a_sixteenth_gossip_option_is_never_read() {
    let mut body = 0xAAu64.to_le_bytes().to_vec();
    body.extend_from_slice(&100u32.to_le_bytes());
    body.extend_from_slice(&16u32.to_le_bytes());
    for i in 1..=16u32 {
        // The 16th option's index is 0.
        body.extend_from_slice(&(i % 16).to_le_bytes());
        body.push(0);
        body.push(2);
        body.extend_from_slice(format!("Option {i}\0").as_bytes());
    }
    body.extend_from_slice(&1u32.to_le_bytes()); // questOptionCount: 1
    body.extend_from_slice(&42u32.to_le_bytes());
    body.extend_from_slice(&7u32.to_le_bytes());
    body.extend_from_slice(&10u32.to_le_bytes());
    body.extend_from_slice(b"A Quest\0");
    match messages::parse_server(messages::opcode::SMSG_GOSSIP_MESSAGE, &body).unwrap() {
        ServerPacket::GossipMessage {
            options, quests, ..
        } => {
            assert_eq!(options.len(), 15);
            assert_eq!(options[14].message, "Option 15");
            assert!(options.iter().all(|o| !o.coded));
            assert!(quests.is_empty());
        }
        other => panic!("gossip message, got {}", other.name()),
    }
}

#[test]
fn vendor_bodies_golden() {
    // CMSG_LIST_INVENTORY (vmangos Item.cpp:94): a full guid.
    assert_eq!(
        messages::list_inventory(0x1234_5678_9abc_def0),
        hx("f0debc9a78563412"),
        "CMSG_LIST_INVENTORY body"
    );

    // CMSG_BUY_ITEM (`0x5e1e90`): vendorGuid, item entry (not muid), count, and the trailing
    // byte `BuyMerchantItem` passes as 1 (`0x4fb933`, written at `0x5e1ee8`-`0x5e1eef`).
    assert_eq!(
        messages::buy_item(0x1234_5678_9abc_def0, 2488, 1),
        hx(concat!("f0debc9a78563412", "b8090000", "01", "01")),
        "CMSG_BUY_ITEM body"
    );

    // CMSG_SELL_ITEM (vmangos Item.cpp:87-92): vendorGuid, itemGuid, count (0 = whole stack).
    assert_eq!(
        messages::sell_item(0x1234_5678_9abc_def0, 0x42, 0),
        hx(concat!("f0debc9a78563412", "4200000000000000", "00")),
        "CMSG_SELL_ITEM body"
    );
}

#[test]
fn gossip_message_wire() {
    use benilla_protocol::messages::{GossipOption, QuestOption};

    // SMSG_GOSSIP_MESSAGE (vmangos GossipDef.cpp:180-225), with no quest options.
    let mut body = 0xAAu64.to_le_bytes().to_vec(); // objectGuid
    body.extend_from_slice(&100u32.to_le_bytes()); // textId
    body.extend_from_slice(&2u32.to_le_bytes()); // optionCount
    body.extend_from_slice(&0u32.to_le_bytes()); // option 0: index
    body.push(0); // icon: chat bubble
    body.push(0); // coded: false
    body.extend_from_slice(b"Train me\0");
    body.extend_from_slice(&1u32.to_le_bytes()); // option 1: index
    body.push(1); // icon: vendor
    body.push(1); // coded: true
    body.extend_from_slice(b"Show me your wares\0");
    body.extend_from_slice(&0u32.to_le_bytes()); // questOptionCount: 0

    match messages::parse_server(messages::opcode::SMSG_GOSSIP_MESSAGE, &body).unwrap() {
        ServerPacket::GossipMessage {
            npc,
            text_id,
            options,
            quests,
        } => {
            assert_eq!((npc, text_id), (0xAA, 100));
            assert_eq!(
                options,
                vec![
                    GossipOption {
                        index: 0,
                        icon: 0,
                        coded: false,
                        message: "Train me".into(),
                    },
                    GossipOption {
                        index: 1,
                        icon: 1,
                        coded: true,
                        message: "Show me your wares".into(),
                    },
                ]
            );
            assert!(quests.is_empty());
        }
        other => panic!("gossip message, got {}", other.name()),
    }

    let packet = messages::parse_server(messages::opcode::SMSG_GOSSIP_MESSAGE, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::GossipMenu {
            npc,
            text_id,
            options,
            quests,
        } => {
            assert_eq!((npc, text_id, options.len()), (0xAA, 100, 2));
            assert!(quests.is_empty());
        }
        other => panic!("gossip menu event, got {other:?}"),
    }

    let mut body_q = 0xBBu64.to_le_bytes().to_vec();
    body_q.extend_from_slice(&200u32.to_le_bytes()); // textId
    body_q.extend_from_slice(&0u32.to_le_bytes()); // optionCount: 0
    body_q.extend_from_slice(&1u32.to_le_bytes()); // questOptionCount: 1
    body_q.extend_from_slice(&42u32.to_le_bytes()); // questId
    body_q.extend_from_slice(&7u32.to_le_bytes()); // icon
    body_q.extend_from_slice(&10u32.to_le_bytes()); // level
    body_q.extend_from_slice(b"A Quest\0");

    match messages::parse_server(messages::opcode::SMSG_GOSSIP_MESSAGE, &body_q).unwrap() {
        ServerPacket::GossipMessage {
            npc,
            options,
            quests,
            ..
        } => {
            assert_eq!(npc, 0xBB);
            assert!(options.is_empty());
            assert_eq!(
                quests,
                vec![QuestOption {
                    quest_id: 42,
                    icon: 7,
                    level: 10,
                    title: "A Quest".into(),
                }]
            );
        }
        other => panic!("gossip message with quests, got {}", other.name()),
    }

    match messages::parse_server(messages::opcode::SMSG_GOSSIP_COMPLETE, &[]).unwrap() {
        ServerPacket::GossipComplete => {}
        other => panic!("gossip complete, got {}", other.name()),
    }
    let packet = messages::parse_server(messages::opcode::SMSG_GOSSIP_COMPLETE, &[]).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::GossipComplete => {}
        other => panic!("gossip complete event, got {other:?}"),
    }
}

/// `SMSG_GOSSIP_POI`, a guard's directions marker (vmangos `GossipDef.cpp:239-295`); the body is
/// `points_of_interest` row 658, "Lion's Pride Inn".
#[test]
fn gossip_poi_wire_and_event() {
    let body = hx(concat!(
        "63000000",                         // u32 flags = 99 (0x63): candidate | in-range icon
        "66cd13c6",                         // f32 x = -9459.35
        "6f522842",                         // f32 y = 42.0805
        "06000000",                         // u32 icon = 6 (ICON_POI_REDFLAG)
        "00000000",                         // u32 data = 0
        "4c696f6e277320507269646520496e6e", // "Lion's Pride Inn"
        "00",                               // … NUL-terminated
    ));

    let expected = messages::GossipPoi {
        flags: 99,
        pos: [-9459.35, 42.0805],
        icon: 6,
        data: 0,
        name: "Lion's Pride Inn".into(),
    };
    match messages::parse_server(messages::opcode::SMSG_GOSSIP_POI, &body).unwrap() {
        ServerPacket::GossipPoi(poi) => assert_eq!(poi, expected),
        other => panic!("gossip poi, got {}", other.name()),
    }
    let packet = messages::parse_server(messages::opcode::SMSG_GOSSIP_POI, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::GossipPoi(poi) => assert_eq!(poi, expected),
        other => panic!("gossip poi event, got {other:?}"),
    }
}

#[test]
fn npc_text_update_greeting_extraction() {
    // SMSG_NPC_TEXT_UPDATE (vmangos GossipDef.cpp:298-369): always 8 blocks of {f32 probability,
    // cstr text0 (male), cstr text1 (female), u32 languageId, 3x(u32 emoteDelay, u32 emoteId)}.
    fn block(w: &mut Vec<u8>, probability: f32, text0: &str, text1: &str) {
        w.extend_from_slice(&probability.to_le_bytes());
        w.extend_from_slice(text0.as_bytes());
        w.push(0);
        w.extend_from_slice(text1.as_bytes());
        w.push(0);
        w.extend_from_slice(&0u32.to_le_bytes()); // languageId
        for _ in 0..3 {
            w.extend_from_slice(&0u32.to_le_bytes()); // emoteDelay
            w.extend_from_slice(&0u32.to_le_bytes()); // emoteId
        }
    }

    // The parse carries all 8 blocks; `select_greeting` picks one.
    let mut body = 321u32.to_le_bytes().to_vec(); // textID
    block(&mut body, 0.1, "Low probability greeting", "");
    block(&mut body, 0.0, "", "");
    block(&mut body, 0.0, "", "");
    block(&mut body, 0.5, "Welcome, $N!", "Welcome, traveler!");
    block(&mut body, 0.0, "", "");
    block(&mut body, 0.0, "", "");
    block(&mut body, 0.2, "", "Female-only greeting");
    block(&mut body, 0.0, "", "");

    match messages::parse_server(messages::opcode::SMSG_NPC_TEXT_UPDATE, &body).unwrap() {
        ServerPacket::NpcText { text_id, blocks } => {
            assert_eq!(text_id, 321);
            assert_eq!(blocks.len(), messages::NPC_TEXT_BLOCKS);
            assert_eq!(blocks[3].probability, 0.5);
            assert_eq!(blocks[3].male, "Welcome, $N!");
            assert_eq!(blocks[3].female, "Welcome, traveler!");
            assert_eq!(blocks[6].female, "Female-only greeting");
        }
        other => panic!("npc text update, got {}", other.name()),
    }

    let packet = messages::parse_server(messages::opcode::SMSG_NPC_TEXT_UPDATE, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::NpcGreeting { text_id, blocks } => {
            assert_eq!(text_id, 321);
            assert_eq!(blocks.len(), messages::NPC_TEXT_BLOCKS);

            // A near-zero threshold draws the first block with text in the gender's column, the
            // full sum the last; the male column skips block 6, the female column block 0.
            const NEAR: f32 = 1.999_999_9;
            const FAR: f32 = 1.0;
            let pick = |gender, roll| messages::select_greeting(&blocks, gender, roll);
            assert_eq!(pick(0, NEAR), Some("Low probability greeting"));
            assert_eq!(pick(0, FAR), Some("Welcome, $N!"));
            assert_eq!(pick(1, NEAR), Some("Welcome, traveler!"));
            assert_eq!(pick(1, FAR), Some("Female-only greeting"));
        }
        other => panic!("npc greeting event, got {other:?}"),
    }
}

/// The greeting draw (reference `0x4e2010`): a weighted random pick over the blocks whose chosen
/// gender column is non-empty.
#[test]
fn greeting_draw_follows_the_weighted_law() {
    use benilla_protocol::messages::{select_greeting, NpcTextBlock};

    let b = |probability: f32, male: &str, female: &str| NpcTextBlock {
        probability,
        male: male.into(),
        female: female.into(),
    };
    // The threshold is (2 - roll) * sum for roll in [1, 2): FAR gives the sum, NEAR nearly 0.
    const NEAR: f32 = 1.999_999_9;
    const FAR: f32 = 1.0;

    // An all-zero record, vmangos's fallback: the threshold is 0 and the test is `<=`, so block 0
    // wins on every roll.
    let zeros: Vec<_> = (0..8).map(|i| b(0.0, &format!("line {i}"), "")).collect();
    for roll in [FAR, 1.5, NEAR] {
        assert_eq!(select_greeting(&zeros, 0, roll), Some("line 0"));
    }

    // The draw walks in order: a near-zero threshold takes the first non-empty block, the full
    // sum the last.
    let weighted = vec![b(0.1, "small", ""), b(0.8, "big", ""), b(0.1, "last", "")];
    assert_eq!(select_greeting(&weighted, 0, NEAR), Some("small"));
    assert_eq!(select_greeting(&weighted, 0, FAR), Some("last"));

    // Genderless (2) reads as male: the reference tests the gender `== 1`, not `!= 0`.
    let gendered = vec![b(1.0, "sir", "madam")];
    assert_eq!(select_greeting(&gendered, 0, FAR), Some("sir"));
    assert_eq!(select_greeting(&gendered, 1, FAR), Some("madam"));
    assert_eq!(
        select_greeting(&gendered, 2, FAR),
        Some("sir"),
        "genderless"
    );

    // No fallback to the other column: a female NPC skips male-only blocks, and if none remain she
    // gets no greeting, the reference's "Missing gossip text!" path.
    let male_only = vec![b(1.0, "men only", ""), b(1.0, "", "ladies")];
    assert_eq!(select_greeting(&male_only, 1, FAR), Some("ladies"));
    assert_eq!(select_greeting(&male_only, 1, NEAR), Some("ladies"));
    assert_eq!(select_greeting(&[b(1.0, "men only", "")], 1, FAR), None);
    assert_eq!(select_greeting(&[], 0, FAR), None, "empty record");
}

#[test]
fn vendor_list_inventory_wire() {
    use benilla_protocol::messages::VendorItem;

    // SMSG_LIST_INVENTORY (vmangos ItemHandler.cpp:741-810): vendorGuid, `u8` count, then per row
    // `u32` muid, entry, displayId, currentCount, price, maxDurability and buyCount.
    let mut body = 0xCCu64.to_le_bytes().to_vec();
    body.push(2); // count
    for v in [1u32, 2488, 22078, 0xFFFF_FFFF, 536, 35, 1] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    for v in [2u32, 2489, 22079, 5, 342, 40, 1] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    body.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]); // trailing junk, must not become a third row

    match messages::parse_server(messages::opcode::SMSG_LIST_INVENTORY, &body).unwrap() {
        ServerPacket::VendorList { vendor, items } => {
            assert_eq!(vendor, 0xCC);
            assert_eq!(
                items,
                vec![
                    VendorItem {
                        slot: 1,
                        entry: 2488,
                        display_id: 22078,
                        current_count: 0xFFFF_FFFF,
                        price: 536,
                        max_durability: 35,
                        buy_count: 1,
                    },
                    VendorItem {
                        slot: 2,
                        entry: 2489,
                        display_id: 22079,
                        current_count: 5,
                        price: 342,
                        max_durability: 40,
                        buy_count: 1,
                    },
                ]
            );
        }
        other => panic!("vendor list, got {}", other.name()),
    }

    let packet = messages::parse_server(messages::opcode::SMSG_LIST_INVENTORY, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::VendorInventory { vendor, items } => {
            assert_eq!((vendor, items.len()), (0xCC, 2));
        }
        other => panic!("vendor inventory event, got {other:?}"),
    }

    // Empty stock: count 0, then an error byte (vmangos ItemHandler.cpp:728-733, 806-809).
    let mut empty_body = 0xDDu64.to_le_bytes().to_vec();
    empty_body.push(0); // count
    empty_body.push(0); // trailing errorByte(0)
    let (packet, tail) =
        messages::parse_server_with_tail(messages::opcode::SMSG_LIST_INVENTORY, &empty_body)
            .unwrap();
    assert_eq!(tail, 0, "the error byte is part of the layout, not a tail");
    match packet {
        ServerPacket::VendorList { vendor, items } => {
            assert_eq!(vendor, 0xDD);
            assert!(items.is_empty());
        }
        other => panic!("empty vendor list, got {}", other.name()),
    }
    // The error byte is required: count 0 with nothing after it is a short read.
    let truncated = &empty_body[..empty_body.len() - 1];
    assert!(messages::parse_server(messages::opcode::SMSG_LIST_INVENTORY, truncated).is_err());
}

/// A `u32` option count of `0xFFFF_FFFF` over one valid option is a short-read `Err`: the decoder
/// neither reserves the count nor aborts.
#[test]
fn a_lying_gossip_option_count_is_a_short_read_not_an_allocation() {
    let mut body = 0x77u64.to_le_bytes().to_vec(); // npc guid
    body.extend_from_slice(&1u32.to_le_bytes()); // text id
    body.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // option count
    body.extend_from_slice(&0u32.to_le_bytes()); // one valid option: index
    body.push(0); // icon
    body.push(0); // coded
    body.extend_from_slice(b"Hello.\0"); // message
    let err = messages::parse_server(messages::opcode::SMSG_GOSSIP_MESSAGE, &body)
        .err()
        .expect("the second option is missing, so the read is short");
    assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
}

#[test]
fn vendor_buy_sell_result_wire() {
    use benilla_protocol::messages::{buy_result, sell_result};

    // SMSG_BUY_ITEM (vmangos Item.cpp:190-196): vendorGuid, 1-based vendorSlot, newCount
    // (0xFFFF_FFFF unlimited), purchaseCount.
    let mut body = 0xCCu64.to_le_bytes().to_vec();
    for v in [1u32, 0xFFFF_FFFF, 1] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    match messages::parse_server(messages::opcode::SMSG_BUY_ITEM, &body).unwrap() {
        ServerPacket::BuyItem {
            vendor,
            slot,
            new_count,
            purchase_count,
        } => {
            assert_eq!(
                (vendor, slot, new_count, purchase_count),
                (0xCC, 1, 0xFFFF_FFFF, 1)
            );
        }
        other => panic!("buy item, got {}", other.name()),
    }
    let packet = messages::parse_server(messages::opcode::SMSG_BUY_ITEM, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::VendorBuyResult { vendor, slot, .. } => {
            assert_eq!((vendor, slot), (0xCC, 1));
        }
        other => panic!("vendor buy result event, got {other:?}"),
    }

    // SMSG_SELL_ITEM (vmangos Item.cpp:183-188), the error path only: vendorGuid, itemGuid, reason.
    let mut body = 0xCCu64.to_le_bytes().to_vec();
    body.extend_from_slice(&0x42u64.to_le_bytes());
    body.push(sell_result::CANT_FIND_ITEM);
    match messages::parse_server(messages::opcode::SMSG_SELL_ITEM, &body).unwrap() {
        ServerPacket::SellItemResult {
            vendor,
            item_guid,
            reason,
        } => {
            assert_eq!(
                (vendor, item_guid, reason),
                (0xCC, 0x42, sell_result::CANT_FIND_ITEM)
            );
        }
        other => panic!("sell item error, got {}", other.name()),
    }
    let packet = messages::parse_server(messages::opcode::SMSG_SELL_ITEM, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::VendorSellFailed { reason, .. } => {
            assert_eq!(reason, sell_result::CANT_FIND_ITEM);
        }
        other => panic!("vendor sell failed event, got {other:?}"),
    }

    // SMSG_BUY_FAILED (vmangos Item.h:277): vendorGuid, itemEntry, reason.
    let mut body = 0xCCu64.to_le_bytes().to_vec();
    body.extend_from_slice(&2488u32.to_le_bytes());
    body.push(buy_result::NOT_ENOUGH_MONEY);
    match messages::parse_server(messages::opcode::SMSG_BUY_FAILED, &body).unwrap() {
        ServerPacket::BuyFailed {
            vendor,
            item_entry,
            reason,
        } => {
            assert_eq!(
                (vendor, item_entry, reason),
                (0xCC, 2488, buy_result::NOT_ENOUGH_MONEY)
            );
        }
        other => panic!("buy failed, got {}", other.name()),
    }
    let packet = messages::parse_server(messages::opcode::SMSG_BUY_FAILED, &body).unwrap();
    match decode(packet).pop().unwrap() {
        SessionEvent::VendorBuyFailed { reason, .. } => {
            assert_eq!(reason, buy_result::NOT_ENOUGH_MONEY);
        }
        other => panic!("vendor buy failed event, got {other:?}"),
    }

    // The result codes (vmangos ItemDefines.h:120-141).
    assert_eq!(
        (
            buy_result::CANT_FIND_ITEM,
            buy_result::ITEM_ALREADY_SOLD,
            buy_result::NOT_ENOUGH_MONEY,
            buy_result::SELLER_DONT_LIKE_YOU,
            buy_result::DISTANCE_TOO_FAR,
            buy_result::ITEM_SOLD_OUT,
            buy_result::CANT_CARRY_MORE,
            buy_result::RANK_REQUIRE,
            buy_result::REPUTATION_REQUIRE,
        ),
        (0, 1, 2, 4, 5, 7, 8, 11, 12)
    );
    assert_eq!(
        (
            sell_result::CANT_FIND_ITEM,
            sell_result::CANT_SELL_ITEM,
            sell_result::CANT_FIND_VENDOR,
            sell_result::YOU_DONT_OWN_THAT_ITEM,
            sell_result::UNK,
            sell_result::ONLY_EMPTY_BAG,
        ),
        (1, 2, 3, 4, 5, 6)
    );
}
