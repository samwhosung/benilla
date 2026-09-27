//! The item wire: the query request and its full template reply, `CMSG_USE_ITEM`'s target forms,
//! equip and open, and the inventory-change refusals.

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages;
use benilla_protocol::ServerPacket;
use common::hx;

#[test]
fn item_query_wire() {
    // CMSG_ITEM_QUERY_SINGLE: `u32` entry and the full item guid (vmangos `QueryItem`).
    assert_eq!(messages::item_query(117, 0), hx("750000000000000000000000"));

    // CMSG_USE_ITEM (vmangos UseItem::ReadFromWorldPacket): bagIndex, slot, spellSlot, then a
    // target block, here self (`u16` mask 0). Backpack slot 1 is bag 255, player slot 23.
    assert_eq!(
        messages::use_item(255, 23, 0, messages::UseItemTarget::SelfImplicit),
        hx("ff17000000")
    );

    // A key in a lock: TARGET_FLAG_GAMEOBJECT (0x0800) and the object's packed guid, as the
    // reference sends it (`0x6e57d8`); vmangos opens a key lock only for a cast item
    // (Spell.cpp:7892). TARGET_FLAG_LOCKED (0x4000) never reaches the wire mask (`0x6e5f69`).
    // Keyring slot 1 is bag 255, player slot 81; the guid's two zero bytes pack to mask 0xDB.
    assert_eq!(
        messages::use_item(
            255,
            81,
            0,
            messages::UseItemTarget::Object(0xF110_000C_1F00_A3B2)
        ),
        hx("ff51000008dbb2a31f0c10f1"),
    );

    // Packed guid 1 is mask 0x01 and one byte; the spellSlot ordinal is the third byte.
    assert_eq!(
        messages::use_item(255, 81, 2, messages::UseItemTarget::Object(1)),
        hx("ff510200080101")
    );

    // A unit target (a bandage, a soulstone): TARGET_FLAG_UNIT (0x0002) and the packed guid, the
    // block CMSG_CAST_SPELL writes; the reference builds both in one sender (`0x6e54f0`).
    assert_eq!(
        messages::use_item(255, 24, 0, messages::UseItemTarget::Unit(1)),
        hx("ff180002000101")
    );

    // A thrown item (dynamite, a grenade): TARGET_FLAG_DEST_LOCATION (0x0040) and three `f32`
    // coords, the tail `cast_spell_at_dest` writes.
    assert_eq!(
        messages::use_item(19, 3, 1, messages::UseItemTarget::Dest([1.0, 2.0, 3.0])),
        hx("13030140000000803f0000004000004040")
    );

    // The source form (0x0020), as for spell 265 on Martin Fury: the reference (`0x6e60f0`) sets
    // 0x0020 where a dest sets 0x0040. Martin Fury is worn, so bag 255, slot 3 (body).
    assert_eq!(
        messages::use_item(255, 3, 0, messages::UseItemTarget::Source([1.0, 2.0, 3.0])),
        hx("ff030020000000803f0000004000004040")
    );

    // An item target (a poison or oil on a weapon): TARGET_FLAG_ITEM (0x0010) and the packed
    // guid, as `cast_spell_on_item` writes (reference `0x6e5b40`); 0xF150_0000_0000_ABCD packs to
    // mask 0xC3 and four bytes.
    assert_eq!(
        messages::use_item(255, 24, 0, messages::UseItemTarget::Item(1)),
        hx("ff180010000101")
    );
    assert_eq!(
        messages::use_item(
            255,
            24,
            0,
            messages::UseItemTarget::Item(0xF150_0000_0000_ABCD)
        ),
        hx("ff18001000c3cdab50f1")
    );

    // A corpse target (Goblin Jumper Cables' Defibrillate, `Targets 0x8000`): the corpse bit and
    // the packed guid, the block `cast_spell_corpse` writes; vmangos reads the guid for either
    // corpse bit (`SpellCastTargetsInfo.cpp:156-157`). 0xF101_0000_0000_002A packs to mask 0xC1.
    assert_eq!(
        messages::use_item(
            255,
            24,
            0,
            messages::UseItemTarget::Corpse(messages::CorpseTarget::Ally, 0xF101_0000_0000_002A)
        ),
        hx("ff18000080c12a01f1")
    );

    // CMSG_AUTOEQUIP_ITEM (vmangos AutoEquipItem::ReadFromWorldPacket): bagIndex, slot.
    assert_eq!(messages::auto_equip_item(255, 25), hx("ff19"));

    // CMSG_OPEN_ITEM (vmangos Server/Packets/Spell.cpp:19-23): bagIndex and slot only, no cast
    // block. A clam, lockbox or gift uses it; the server answers with loot on the item's guid.
    // An item in an equipped bag addresses that bag's player slot (19..22) and its inner slot.
    assert_eq!(messages::open_item(255, 25), hx("ff19"));
    assert_eq!(messages::open_item(19, 0), hx("1300"));

    // SMSG_INVENTORY_CHANGE_FAILURE (vmangos InventoryChangeFailure::AppendBodyTo): the level
    // refusal (reason 1) carries a u32 required level before the guids; others don't.
    let level_fail = messages::parse_server(
        messages::opcode::SMSG_INVENTORY_CHANGE_FAILURE,
        &hx("010a000000420000000000004000000000000000000000"),
    )
    .unwrap();
    match level_fail {
        ServerPacket::InventoryChangeFailure {
            reason,
            required_level,
            item_guid,
            bag_slot,
        } => {
            assert_eq!((reason, required_level), (1, Some(10)));
            assert_eq!(item_guid, 0x4000_0000_0000_0042);
            // The trailing byte is the destination bag's absolute player slot, not a subslot.
            assert_eq!(bag_slot, 0);
        }
        other => panic!("level refusal, got {}", other.name()),
    }
    let plain_fail = messages::parse_server(
        messages::opcode::SMSG_INVENTORY_CHANGE_FAILURE,
        &hx("14420000000000004000000000000000000000"),
    )
    .unwrap();
    match plain_fail {
        ServerPacket::InventoryChangeFailure {
            reason,
            required_level,
            ..
        } => assert_eq!((reason, required_level), (0x14, None)),
        other => panic!("plain refusal, got {}", other.name()),
    }

    // SMSG_MESSAGECHAT's system shape (type 0x0A: guid, length-prefixed text, tag), the shape
    // GM commands such as `.additem` answer with.
    let sys = messages::parse_server(
        messages::opcode::SMSG_MESSAGECHAT,
        &hx("0a0000000000000000000000000600000068656c6c6f0000"),
    )
    .unwrap();
    match sys {
        ServerPacket::MessageChat(m) => {
            assert_eq!((m.chat_type, m.text.as_str()), (0x0a, "hello"));
            assert_eq!((m.sender_guid, m.sender_name, m.channel), (0, None, None));
        }
        other => panic!("system chat, got {}", other.name()),
    }
}

/// `SMSG_ITEM_QUERY_SINGLE_RESPONSE` in vmangos order (`ItemHandler.cpp:269-415`, build 5875
/// branches), for a sword whose fields all differ so a misaligned read fails.
#[test]
fn item_query_response_full_weapon_golden() {
    use benilla_protocol::messages::{ItemDamage, ItemInfo, ItemSpellEntry, ItemUseSpell};

    let mut body = 12_345u32.to_le_bytes().to_vec(); // entry
    body.extend_from_slice(&2u32.to_le_bytes()); // class: weapon
    body.extend_from_slice(&7u32.to_le_bytes()); // subclass: sword
    body.extend_from_slice(b"Verified Blade of Testing\0");
    body.extend_from_slice(&[0, 0, 0]); // name2..4: empty cstrings
    body.extend_from_slice(&6303u32.to_le_bytes()); // displayInfoID
    body.extend_from_slice(&4u32.to_le_bytes()); // quality: epic
    body.extend_from_slice(&66u32.to_le_bytes()); // flags
    body.extend_from_slice(&4200u32.to_le_bytes()); // buyPrice
    body.extend_from_slice(&1050u32.to_le_bytes()); // sellPrice
    body.extend_from_slice(&21u32.to_le_bytes()); // inventoryType: WEAPONMAINHAND
    body.extend_from_slice(&(-1i32).to_le_bytes()); // AllowableClass: all classes
    body.extend_from_slice(&3i32.to_le_bytes()); // AllowableRace
    body.extend_from_slice(&60u32.to_le_bytes()); // ItemLevel
    body.extend_from_slice(&55u32.to_le_bytes()); // RequiredLevel
    body.extend_from_slice(&164u32.to_le_bytes()); // RequiredSkill
    body.extend_from_slice(&225u32.to_le_bytes()); // RequiredSkillRank
    body.extend_from_slice(&5209u32.to_le_bytes()); // RequiredSpell
    body.extend_from_slice(&3u32.to_le_bytes()); // RequiredHonorRank
    body.extend_from_slice(&2u32.to_le_bytes()); // RequiredCityRank
    body.extend_from_slice(&69u32.to_le_bytes()); // RequiredReputationFaction
    body.extend_from_slice(&4u32.to_le_bytes()); // RequiredReputationRank
    body.extend_from_slice(&5u32.to_le_bytes()); // MaxCount
    body.extend_from_slice(&1u32.to_le_bytes()); // Stackable
    body.extend_from_slice(&0u32.to_le_bytes()); // ContainerSlots

    // 10 ItemStat { type, value }: empty ones are dropped; the negative one checks signed reads.
    let stat_slots: [(u32, i32); 10] = [
        (4, 15), // STRENGTH +15
        (7, 20), // STAMINA +20
        (5, -3), // INTELLECT -3
        (0, 0),
        (0, 0),
        (0, 0),
        (0, 0),
        (0, 0),
        (0, 0),
        (0, 0),
    ];
    for (stat_type, stat_value) in stat_slots {
        body.extend_from_slice(&stat_type.to_le_bytes());
        body.extend_from_slice(&stat_value.to_le_bytes());
    }

    // 5 Damage { f32 min, f32 max, u32 type }: blocks with `max > 0` are kept, and block 0 also
    // fills dmg_min/dmg_max/dmg_type.
    let dmg_slots: [(f32, f32, u32); 5] = [
        (12.0, 22.0, 0), // physical
        (3.0, 7.0, 2),   // Fire
        (0.0, 0.0, 0),
        (0.0, 0.0, 0),
        (0.0, 0.0, 0),
    ];
    for (min, max, school) in dmg_slots {
        body.extend_from_slice(&min.to_le_bytes());
        body.extend_from_slice(&max.to_le_bytes());
        body.extend_from_slice(&school.to_le_bytes());
    }

    body.extend_from_slice(&15u32.to_le_bytes()); // Armor
                                                  // Holy, Fire, Nature, Frost, Shadow, Arcane.
    for r in [0i32, 8, 0, 0, 12, 0] {
        body.extend_from_slice(&r.to_le_bytes());
    }
    body.extend_from_slice(&2800u32.to_le_bytes()); // Delay (ms)
    body.extend_from_slice(&2u32.to_le_bytes()); // AmmoType
    body.extend_from_slice(&1.5f32.to_le_bytes()); // RangedModRange

    // 5 spell blocks { id, trigger, charges, cooldown, category, categoryCooldown }: ON_USE with
    // a -1 cooldown beside a real category (a mix vmangos sends), ON_EQUIP, then the server's
    // empty-slot sentinel (0, 0, 0, -1, 0, -1).
    let spell_slots: [(u32, u32, i32, i32, u32, i32); 5] = [
        (17_251, 0, 0, -1, 4, 60_000),
        (671, 1, 0, 0, 0, 0),
        (0, 0, 0, -1, 0, -1),
        (0, 0, 0, -1, 0, -1),
        (0, 0, 0, -1, 0, -1),
    ];
    for (spell_id, trigger, charges, cooldown_ms, category, category_cooldown_ms) in spell_slots {
        body.extend_from_slice(&spell_id.to_le_bytes());
        body.extend_from_slice(&trigger.to_le_bytes());
        body.extend_from_slice(&charges.to_le_bytes());
        body.extend_from_slice(&cooldown_ms.to_le_bytes());
        body.extend_from_slice(&category.to_le_bytes());
        body.extend_from_slice(&category_cooldown_ms.to_le_bytes());
    }

    body.extend_from_slice(&2u32.to_le_bytes()); // Bonding: BIND_WHEN_EQUIPPED
    body.extend_from_slice(b"A blade of pure verification.\0"); // Description
    body.extend_from_slice(&333u32.to_le_bytes()); // PageText
    body.extend_from_slice(&7u32.to_le_bytes()); // LanguageID
    body.extend_from_slice(&2u32.to_le_bytes()); // PageMaterial
    body.extend_from_slice(&444u32.to_le_bytes()); // StartQuest
    body.extend_from_slice(&12u32.to_le_bytes()); // LockID
    body.extend_from_slice(&3u32.to_le_bytes()); // Material
    body.extend_from_slice(&3u32.to_le_bytes()); // Sheath: one-handed
    body.extend_from_slice(&87u32.to_le_bytes()); // RandomProperty
    body.extend_from_slice(&18u32.to_le_bytes()); // Block
    body.extend_from_slice(&25u32.to_le_bytes()); // ItemSet
    body.extend_from_slice(&120u32.to_le_bytes()); // MaxDurability
    body.extend_from_slice(&8u32.to_le_bytes()); // Area
    body.extend_from_slice(&1u32.to_le_bytes()); // Map
    body.extend_from_slice(&4u32.to_le_bytes()); // BagFamily

    match messages::parse_server(messages::opcode::SMSG_ITEM_QUERY_SINGLE_RESPONSE, &body).unwrap()
    {
        ServerPacket::ItemQueryResponse { entry, info } => {
            assert_eq!(entry, 12_345);
            assert_eq!(
                info,
                Some(Box::new(ItemInfo {
                    class: 2,
                    subclass: 7,
                    name: "Verified Blade of Testing".into(),
                    display_info_id: 6303,
                    quality: 4,
                    flags: 66,
                    buy_price: 4200,
                    sell_price: 1050,
                    inventory_type: 21,
                    allowable_class: -1,
                    allowable_race: 3,
                    item_level: 60,
                    required_level: 55,
                    required_skill: 164,
                    required_skill_rank: 225,
                    required_spell: 5209,
                    required_honor_rank: 3,
                    required_city_rank: 2,
                    required_rep_faction: 69,
                    required_rep_rank: 4,
                    max_count: 5,
                    stackable: 1,
                    container_slots: 0,
                    stats: vec![(4, 15), (7, 20), (5, -3)],
                    damages: vec![
                        ItemDamage {
                            min: 12.0,
                            max: 22.0,
                            school: 0
                        },
                        ItemDamage {
                            min: 3.0,
                            max: 7.0,
                            school: 2
                        },
                    ],
                    dmg_min: 12.0,
                    dmg_max: 22.0,
                    dmg_type: 0,
                    armor: 15,
                    resistances: [0, 8, 0, 0, 12, 0],
                    delay_ms: 2800,
                    ammo_type: 2,
                    ranged_mod_range: 1.5,
                    spell_charges_0: 0,
                    spells: vec![
                        ItemSpellEntry {
                            index: 0,
                            spell_id: 17_251,
                            trigger: 0,
                            charges: 0,
                            cooldown_ms: -1,
                            category: 4,
                            category_cooldown_ms: 60_000,
                        },
                        ItemSpellEntry {
                            index: 1,
                            spell_id: 671,
                            trigger: 1,
                            charges: 0,
                            cooldown_ms: 0,
                            category: 0,
                            category_cooldown_ms: 0,
                        },
                    ],
                    use_spell: Some(ItemUseSpell {
                        spell_id: 17_251,
                        cooldown_ms: -1,
                        category: 4,
                        category_cooldown_ms: 60_000,
                    }),
                    bonding: 2,
                    description: "A blade of pure verification.".into(),
                    page_text: 333,
                    language_id: 7,
                    page_material: 2,
                    start_quest: 444,
                    lock_id: 12,
                    material: 3,
                    sheath: 3,
                    random_property: 87,
                    block: 18,
                    item_set: 25,
                    max_durability: 120,
                    area: 8,
                    map: 1,
                    bag_family: 4,
                }))
            );
        }
        other => panic!("item query hit, got {}", other.name()),
    }
}

/// A potion: zeroed stats, damages and spell slots land as empty `Vec`s while the ON_USE slot
/// fills both `spells` and `use_spell`. The server zeroes a consumable's subclass (vmangos
/// `ItemHandler.cpp:300`).
#[test]
fn item_query_response_consumable_all_zero_slots_are_empty_not_garbage() {
    use benilla_protocol::messages::{ItemInfo, ItemSpellEntry, ItemUseSpell};

    let mut body = 117u32.to_le_bytes().to_vec(); // entry
    body.extend_from_slice(&0u32.to_le_bytes()); // class: consumable
    body.extend_from_slice(&0u32.to_le_bytes()); // subclass: server-zeroed for consumables
    body.extend_from_slice(b"Minor Healing Potion\0");
    body.extend_from_slice(&[0, 0, 0]); // name2..4: empty cstrings
    body.extend_from_slice(&1712u32.to_le_bytes()); // displayInfoID
    body.extend_from_slice(&1u32.to_le_bytes()); // quality: common
    body.extend_from_slice(&0u32.to_le_bytes()); // flags
    body.extend_from_slice(&40u32.to_le_bytes()); // buyPrice
    body.extend_from_slice(&10u32.to_le_bytes()); // sellPrice
    body.extend_from_slice(&0u32.to_le_bytes()); // inventoryType: none
    body.extend_from_slice(&(-1i32).to_le_bytes()); // AllowableClass: all classes
    body.extend_from_slice(&(-1i32).to_le_bytes()); // AllowableRace: all races
    body.extend_from_slice(&1u32.to_le_bytes()); // ItemLevel
    for _ in 0..6 {
        body.extend_from_slice(&0u32.to_le_bytes()); // RequiredLevel .. RequiredCityRank
    }
    body.extend_from_slice(&0u32.to_le_bytes()); // RequiredReputationFaction
    body.extend_from_slice(&0u32.to_le_bytes()); // RequiredReputationRank
    body.extend_from_slice(&20u32.to_le_bytes()); // MaxCount
    body.extend_from_slice(&20u32.to_le_bytes()); // Stackable
    body.extend_from_slice(&0u32.to_le_bytes()); // ContainerSlots
    for _ in 0..20 {
        body.extend_from_slice(&0u32.to_le_bytes()); // 10x ItemStat { type, value }: all empty
    }
    for _ in 0..5 {
        body.extend_from_slice(&0f32.to_le_bytes());
        body.extend_from_slice(&0f32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes()); // 5x Damage: all empty
    }
    body.extend_from_slice(&0u32.to_le_bytes()); // Armor
    for _ in 0..6 {
        body.extend_from_slice(&0u32.to_le_bytes()); // resistances: all empty
    }
    body.extend_from_slice(&0u32.to_le_bytes()); // Delay
    body.extend_from_slice(&0u32.to_le_bytes()); // AmmoType
    body.extend_from_slice(&0f32.to_le_bytes()); // RangedModRange
                                                 // Slot 0: the ON_USE heal; 1-4: empty sentinels.
    body.extend_from_slice(&2024u32.to_le_bytes()); // SpellId
    body.extend_from_slice(&0u32.to_le_bytes()); // SpellTrigger: ON_USE
    body.extend_from_slice(&0u32.to_le_bytes()); // SpellCharges
    body.extend_from_slice(&(-1i32).to_le_bytes()); // Cooldown: the spell's own
    body.extend_from_slice(&0u32.to_le_bytes()); // Category
    body.extend_from_slice(&(-1i32).to_le_bytes()); // CategoryCooldown: the spell's own
    for _ in 0..4 {
        body.extend_from_slice(&0u32.to_le_bytes()); // SpellId
        body.extend_from_slice(&0u32.to_le_bytes()); // SpellTrigger
        body.extend_from_slice(&0u32.to_le_bytes()); // SpellCharges
        body.extend_from_slice(&(-1i32).to_le_bytes()); // Cooldown
        body.extend_from_slice(&0u32.to_le_bytes()); // Category
        body.extend_from_slice(&(-1i32).to_le_bytes()); // CategoryCooldown
    }
    body.extend_from_slice(&0u32.to_le_bytes()); // Bonding
    body.push(0); // Description: empty cstring
    for _ in 0..6 {
        body.extend_from_slice(&0u32.to_le_bytes()); // PageText .. Material
    }
    body.extend_from_slice(&0u32.to_le_bytes()); // Sheath
    body.extend_from_slice(&0u32.to_le_bytes()); // RandomProperty
    body.extend_from_slice(&0u32.to_le_bytes()); // Block
    body.extend_from_slice(&0u32.to_le_bytes()); // ItemSet
    body.extend_from_slice(&0u32.to_le_bytes()); // MaxDurability: consumables have none
    body.extend_from_slice(&0u32.to_le_bytes()); // Area
    body.extend_from_slice(&0u32.to_le_bytes()); // Map
    body.extend_from_slice(&0u32.to_le_bytes()); // BagFamily

    match messages::parse_server(messages::opcode::SMSG_ITEM_QUERY_SINGLE_RESPONSE, &body).unwrap()
    {
        ServerPacket::ItemQueryResponse { entry, info } => {
            assert_eq!(entry, 117);
            assert_eq!(
                info,
                Some(Box::new(ItemInfo {
                    class: 0,
                    subclass: 0,
                    name: "Minor Healing Potion".into(),
                    display_info_id: 1712,
                    quality: 1,
                    flags: 0,
                    buy_price: 40,
                    sell_price: 10,
                    inventory_type: 0,
                    allowable_class: -1,
                    allowable_race: -1,
                    item_level: 1,
                    required_level: 0,
                    required_skill: 0,
                    required_skill_rank: 0,
                    required_spell: 0,
                    required_honor_rank: 0,
                    required_city_rank: 0,
                    required_rep_faction: 0,
                    required_rep_rank: 0,
                    max_count: 20,
                    stackable: 20,
                    container_slots: 0,
                    stats: Vec::new(),
                    damages: Vec::new(),
                    dmg_min: 0.0,
                    dmg_max: 0.0,
                    dmg_type: 0,
                    armor: 0,
                    resistances: [0; 6],
                    delay_ms: 0,
                    ammo_type: 0,
                    ranged_mod_range: 0.0,
                    spell_charges_0: 0,
                    spells: vec![ItemSpellEntry {
                        index: 0,
                        spell_id: 2024,
                        trigger: 0,
                        charges: 0,
                        cooldown_ms: -1,
                        category: 0,
                        category_cooldown_ms: -1,
                    }],
                    use_spell: Some(ItemUseSpell {
                        spell_id: 2024,
                        cooldown_ms: -1,
                        category: 0,
                        category_cooldown_ms: -1,
                    }),
                    bonding: 0,
                    description: String::new(),
                    page_text: 0,
                    language_id: 0,
                    page_material: 0,
                    start_quest: 0,
                    lock_id: 0,
                    material: 0,
                    sheath: 0,
                    random_property: 0,
                    block: 0,
                    item_set: 0,
                    max_durability: 0,
                    area: 0,
                    map: 0,
                    bag_family: 0,
                }))
            );
        }
        other => panic!("item query hit, got {}", other.name()),
    }
}

/// A miss is the lone entry with the top bit set (vmangos's unknown-entry branch).
#[test]
fn item_query_response_miss() {
    let fail = messages::parse_server(
        messages::opcode::SMSG_ITEM_QUERY_SINGLE_RESPONSE,
        &hx("d2040080"),
    )
    .unwrap();
    match decode(fail).pop().unwrap() {
        SessionEvent::ItemTemplate { entry, info } => {
            assert_eq!((entry, info), (1234, None));
        }
        _ => panic!("item query miss event"),
    }
}
