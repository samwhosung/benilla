//! Player-to-player trade wire: the trade window's request verbs and the two status packets the
//! server pushes back (vmangos `TradeHandler.cpp`, `Server/Packets/Trade.cpp`).

use std::io;

use crate::wire::{read_i32_le, read_u32_le, read_u64_le, read_u8};

/// Slots per side (`TradeData.h`): six traded plus the non-traded enchant slot.
pub const TRADE_SLOT_COUNT: usize = 7;
/// Slots 0..6, whose items change hands.
pub const TRADE_SLOT_TRADED_COUNT: usize = 6;
/// UI slot 7: its item stays with its owner as the target of an enchant or lockpick spell.
pub const TRADE_SLOT_NONTRADED: usize = 6;

/// `SMSG_TRADE_STATUS` codes (`SharedDefines.h` `enum TradeStatus`), each with its tail inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeStatus {
    /// The target is busy, or the initiator is already trading.
    Busy,
    /// To the target of `CMSG_INITIATE_TRADE`; its `CMSG_BEGIN_TRADE` opens both windows.
    BeginTrade { partner: u64 },
    /// Opens the window on both sides (the client's `TRADE_SHOW`).
    OpenWindow,
    /// The trade was cancelled; both windows close.
    Canceled,
    /// The partner pressed Trade (the accept highlight on their column).
    Accept,
    /// A second busy code, handled as `Busy`.
    Busy2,
    /// No such target for `CMSG_INITIATE_TRADE`.
    NoTarget,
    /// An offer changed after an accept, or the 200 ms scam delay bounced one: the accept drops.
    BackToTrade,
    /// Both sides accepted and the swap completed; both windows close.
    Complete,
    /// The trade was rejected.
    Rejected,
    /// Out of `TRADE_DISTANCE`; also the refusal to initiate while flying or off-map.
    TargetTooFar,
    /// Cross-faction, unless the server allows two-side interaction.
    WrongFaction,
    /// The tail is usually zero; the `u8` vmangos writes between the two fields is dropped.
    CloseWindow {
        result: u32,
        item_limit_category: u32,
    },
    /// Per a vmangos note, handled as `Canceled`; kept distinct so the code round-trips.
    Unknown13,
    /// The target has you on ignore.
    IgnoreYou,
    /// You are stunned.
    YouStunned,
    /// The target is stunned.
    TargetStunned,
    /// You are dead.
    YouDead,
    /// The target is dead.
    TargetDead,
    /// You are logging out.
    YouLogout,
    /// The target is logging out.
    TargetLogout,
    /// A trial account restriction.
    TrialAccount,
    /// "You can only trade conjured items"; `slot` is the offending trade slot.
    OnlyConjured { slot: u8 },
    /// A code outside 0..=22, which vmangos never sends; read with no tail.
    Unknown(u32),
}

impl TradeStatus {
    /// The `u32` status code on the wire.
    pub fn code(self) -> u32 {
        match self {
            TradeStatus::Busy => 0,
            TradeStatus::BeginTrade { .. } => 1,
            TradeStatus::OpenWindow => 2,
            TradeStatus::Canceled => 3,
            TradeStatus::Accept => 4,
            TradeStatus::Busy2 => 5,
            TradeStatus::NoTarget => 6,
            TradeStatus::BackToTrade => 7,
            TradeStatus::Complete => 8,
            TradeStatus::Rejected => 9,
            TradeStatus::TargetTooFar => 10,
            TradeStatus::WrongFaction => 11,
            TradeStatus::CloseWindow { .. } => 12,
            TradeStatus::Unknown13 => 13,
            TradeStatus::IgnoreYou => 14,
            TradeStatus::YouStunned => 15,
            TradeStatus::TargetStunned => 16,
            TradeStatus::YouDead => 17,
            TradeStatus::TargetDead => 18,
            TradeStatus::YouLogout => 19,
            TradeStatus::TargetLogout => 20,
            TradeStatus::TrialAccount => 21,
            TradeStatus::OnlyConjured { .. } => 22,
            TradeStatus::Unknown(code) => code,
        }
    }
}

/// One trade slot's 60-byte item block; an all-zero block is an empty slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TradeItem {
    /// `Item.dbc` entry (`OBJECT_FIELD_ENTRY`), the key for name and icon via `ITEM_QUERY_SINGLE`.
    pub entry: u32,
    pub display_id: u32,
    pub count: u32,
    /// A wrapped gift: the client hides stats and shows the gift-creator name.
    pub wrapped: bool,
    pub gift_creator: u64,
    pub perm_enchant: u32,
    pub creator: u64,
    /// Signed: a negative count means N uses left on some items.
    pub charges: i32,
    pub suffix_factor: u32,
    pub random_prop_id: u32,
    pub lock_id: u32,
    pub max_durability: u32,
    pub durability: u32,
}

/// `SMSG_TRADE_STATUS_EXTENDED`: one side's snapshot, sent whenever that side's offer changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TradeStatusExtended {
    /// The partner's (right) column when the wire byte is 1, ours (left) when 0.
    pub their_window: bool,
    /// The first `u32` after the side byte (`0x5d4990`): the client drops a status for its own
    /// column whose sequence is below its own-edit counter. vmangos writes the slot count there,
    /// 7, whatever the sender's edits (`TradeHandler.cpp:60`).
    pub sequence: u32,
    /// In copper.
    pub gold: u32,
    /// The spell cast on this side's non-traded slot item, 0 for none.
    pub enchant_spell_id: u32,
    pub slots: [Option<TradeItem>; TRADE_SLOT_COUNT],
}

// ── Client → server bodies ──

/// `CMSG_INITIATE_TRADE`: a refusal is a status back to us; success sends the target `BeginTrade`.
pub fn initiate_trade(target: u64) -> Vec<u8> {
    target.to_le_bytes().to_vec()
}

/// `CMSG_ACCEPT_TRADE`: a `u32` the server skips; the 1.12 client sends 1.
pub fn accept_trade() -> Vec<u8> {
    1u32.to_le_bytes().to_vec()
}

/// `CMSG_SET_TRADE_ITEM`: puts the item at (`bag`, `slot`) into `trade_slot`.
pub fn set_trade_item(trade_slot: u8, bag: u8, slot: u8) -> Vec<u8> {
    vec![trade_slot, bag, slot]
}

/// `CMSG_CLEAR_TRADE_ITEM` body.
pub fn clear_trade_item(trade_slot: u8) -> Vec<u8> {
    vec![trade_slot]
}

/// `CMSG_SET_TRADE_GOLD` body, in copper.
pub fn set_trade_gold(copper: u32) -> Vec<u8> {
    copper.to_le_bytes().to_vec()
}

// `CMSG_BEGIN_TRADE`, `CMSG_BUSY_TRADE`, `CMSG_IGNORE_TRADE`, `CMSG_UNACCEPT_TRADE` and
// `CMSG_CANCEL_TRADE` have empty bodies (vmangos `NullClientPacket`), so they need no builder.

// ── Server → client parses ──

/// `SMSG_TRADE_STATUS` (vmangos `TradeStatus::AppendBodyTo`): a `u32` code, then its tail.
pub(super) fn read_trade_status(r: &mut &[u8]) -> io::Result<TradeStatus> {
    let code = read_u32_le(r)?;
    Ok(match code {
        0 => TradeStatus::Busy,
        1 => TradeStatus::BeginTrade {
            partner: read_u64_le(r)?,
        },
        2 => TradeStatus::OpenWindow,
        3 => TradeStatus::Canceled,
        4 => TradeStatus::Accept,
        5 => TradeStatus::Busy2,
        6 => TradeStatus::NoTarget,
        7 => TradeStatus::BackToTrade,
        8 => TradeStatus::Complete,
        9 => TradeStatus::Rejected,
        10 => TradeStatus::TargetTooFar,
        11 => TradeStatus::WrongFaction,
        12 => {
            let result = read_u32_le(r)?;
            let _unk = read_u8(r)?; // vmangos writes it; carries nothing for player trade
            let item_limit_category = read_u32_le(r)?;
            TradeStatus::CloseWindow {
                result,
                item_limit_category,
            }
        }
        13 => TradeStatus::Unknown13,
        14 => TradeStatus::IgnoreYou,
        15 => TradeStatus::YouStunned,
        16 => TradeStatus::TargetStunned,
        17 => TradeStatus::YouDead,
        18 => TradeStatus::TargetDead,
        19 => TradeStatus::YouLogout,
        20 => TradeStatus::TargetLogout,
        21 => TradeStatus::TrialAccount,
        22 => TradeStatus::OnlyConjured { slot: read_u8(r)? },
        other => TradeStatus::Unknown(other),
    })
}

/// `SMSG_TRADE_STATUS_EXTENDED` (vmangos `WorldSession::SendUpdateTrade`): `u8 which`, the
/// sequence, the slot count, gold, enchant spell, then seven records of `u8 index` and a 60-byte
/// item block.
pub(super) fn read_trade_status_extended(r: &mut &[u8]) -> io::Result<TradeStatusExtended> {
    let which = read_u8(r)?;
    let sequence = read_u32_le(r)?;
    let _slot_count = read_u32_le(r)?;
    let gold = read_u32_le(r)?;
    let enchant_spell_id = read_u32_le(r)?;

    let mut slots: [Option<TradeItem>; TRADE_SLOT_COUNT] = [None; TRADE_SLOT_COUNT];
    for _ in 0..TRADE_SLOT_COUNT {
        let index = read_u8(r)? as usize;
        let entry = read_u32_le(r)?;
        let display_id = read_u32_le(r)?;
        let count = read_u32_le(r)?;
        let wrapped = read_u32_le(r)? != 0;
        let gift_creator = read_u64_le(r)?;
        let perm_enchant = read_u32_le(r)?;
        let creator = read_u64_le(r)?;
        let charges = read_i32_le(r)?;
        let suffix_factor = read_u32_le(r)?;
        let random_prop_id = read_u32_le(r)?;
        let lock_id = read_u32_le(r)?;
        let max_durability = read_u32_le(r)?;
        let durability = read_u32_le(r)?;
        if entry != 0 && index < TRADE_SLOT_COUNT {
            slots[index] = Some(TradeItem {
                entry,
                display_id,
                count,
                wrapped,
                gift_creator,
                perm_enchant,
                creator,
                charges,
                suffix_factor,
                random_prop_id,
                lock_id,
                max_durability,
                durability,
            });
        }
    }
    Ok(TradeStatusExtended {
        their_window: which == 1,
        sequence,
        gold,
        enchant_spell_id,
        slots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── CMSG body goldens ──

    #[test]
    fn initiate_trade_is_the_target_guid_le() {
        assert_eq!(
            initiate_trade(0x1122_3344_5566_7788),
            vec![0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11]
        );
    }

    #[test]
    fn accept_trade_is_u32_one() {
        assert_eq!(accept_trade(), vec![1, 0, 0, 0]);
    }

    #[test]
    fn set_trade_item_is_three_u8s() {
        assert_eq!(set_trade_item(2, 0, 23), vec![2, 0, 23]);
    }

    #[test]
    fn clear_trade_item_is_one_u8() {
        assert_eq!(clear_trade_item(5), vec![5]);
    }

    #[test]
    fn set_trade_gold_is_u32_le() {
        assert_eq!(set_trade_gold(0x0001_E240), vec![0x40, 0xE2, 0x01, 0x00]);
    }

    // ── SMSG_TRADE_STATUS parse goldens ──

    /// Every code without a tail reads as its variant and leaves the byte after the code unread.
    #[test]
    fn trade_status_bare_code_has_no_tail() {
        for (code, want) in [
            (0, TradeStatus::Busy),
            (2, TradeStatus::OpenWindow),
            (3, TradeStatus::Canceled),
            (4, TradeStatus::Accept),
            (5, TradeStatus::Busy2),
            (6, TradeStatus::NoTarget),
            (7, TradeStatus::BackToTrade),
            (8, TradeStatus::Complete),
            (9, TradeStatus::Rejected),
            (10, TradeStatus::TargetTooFar),
            (11, TradeStatus::WrongFaction),
            (13, TradeStatus::Unknown13),
            (14, TradeStatus::IgnoreYou),
            (15, TradeStatus::YouStunned),
            (16, TradeStatus::TargetStunned),
            (17, TradeStatus::YouDead),
            (18, TradeStatus::TargetDead),
            (19, TradeStatus::YouLogout),
            (20, TradeStatus::TargetLogout),
            (21, TradeStatus::TrialAccount),
        ] {
            let mut buf = u32::to_le_bytes(code).to_vec();
            buf.push(0xEE); // the next byte, which a bare status leaves unread
            let mut r = &buf[..];
            assert_eq!(read_trade_status(&mut r).unwrap(), want, "code {code}");
            assert_eq!(
                r,
                [0xEE],
                "no tail should be consumed for bare status {code}"
            );
            assert_eq!(want.code(), code);
        }
    }

    #[test]
    fn trade_status_begin_trade_reads_the_partner_guid() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_le_bytes()); // BEGIN_TRADE
        buf.extend_from_slice(&0xDEAD_BEEF_0000_0001u64.to_le_bytes());
        let mut r = &buf[..];
        assert_eq!(
            read_trade_status(&mut r).unwrap(),
            TradeStatus::BeginTrade {
                partner: 0xDEAD_BEEF_0000_0001
            }
        );
        assert!(r.is_empty());
    }

    #[test]
    fn trade_status_close_window_consumes_u32_u8_u32() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&12u32.to_le_bytes()); // CLOSE_WINDOW
        buf.extend_from_slice(&7u32.to_le_bytes()); // result
        buf.push(0); // unk (dropped)
        buf.extend_from_slice(&3u32.to_le_bytes()); // itemLimitCategory
        let mut r = &buf[..];
        assert_eq!(
            read_trade_status(&mut r).unwrap(),
            TradeStatus::CloseWindow {
                result: 7,
                item_limit_category: 3
            }
        );
        assert!(r.is_empty(), "the full u32+u8+u32 tail must be consumed");
    }

    #[test]
    fn trade_status_only_conjured_reads_the_slot_byte() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&22u32.to_le_bytes()); // ONLY_CONJURED
        buf.push(4); // slot
        let mut r = &buf[..];
        assert_eq!(
            read_trade_status(&mut r).unwrap(),
            TradeStatus::OnlyConjured { slot: 4 }
        );
        assert!(r.is_empty());
    }

    #[test]
    fn trade_status_out_of_range_is_unknown_no_tail() {
        let buf = 99u32.to_le_bytes();
        let mut r = &buf[..];
        assert_eq!(read_trade_status(&mut r).unwrap(), TradeStatus::Unknown(99));
        assert!(r.is_empty());
    }

    // ── SMSG_TRADE_STATUS_EXTENDED parse golden ──

    /// A 444-byte snapshot as vmangos builds it; `Some(entry)` fills a slot, `None` zeroes it.
    fn extended_wire(
        which: u8,
        gold: u32,
        spell: u32,
        entries: [Option<u32>; TRADE_SLOT_COUNT],
    ) -> Vec<u8> {
        let mut b = Vec::new();
        b.push(which);
        b.extend_from_slice(&(TRADE_SLOT_COUNT as u32).to_le_bytes()); // sequence
        b.extend_from_slice(&(TRADE_SLOT_COUNT as u32).to_le_bytes()); // slot count
        b.extend_from_slice(&gold.to_le_bytes());
        b.extend_from_slice(&spell.to_le_bytes());
        for (i, entry) in entries.iter().enumerate() {
            b.push(i as u8);
            match entry {
                Some(e) => {
                    b.extend_from_slice(&e.to_le_bytes()); // entry
                    b.extend_from_slice(&11u32.to_le_bytes()); // display_id
                    b.extend_from_slice(&5u32.to_le_bytes()); // count
                    b.extend_from_slice(&0u32.to_le_bytes()); // wrapped
                    b.extend_from_slice(&0u64.to_le_bytes()); // gift_creator
                    b.extend_from_slice(&0u32.to_le_bytes()); // perm_enchant
                    b.extend_from_slice(&0u64.to_le_bytes()); // creator
                    b.extend_from_slice(&(-3i32).to_le_bytes()); // charges (signed)
                    b.extend_from_slice(&0u32.to_le_bytes()); // suffix_factor
                    b.extend_from_slice(&0u32.to_le_bytes()); // random_prop_id
                    b.extend_from_slice(&0u32.to_le_bytes()); // lock_id
                    b.extend_from_slice(&100u32.to_le_bytes()); // max_durability
                    b.extend_from_slice(&80u32.to_le_bytes()); // durability
                }
                None => b.extend_from_slice(&[0u8; 60]),
            }
        }
        b
    }

    /// The parser reads exactly 444 bytes, whether the slots are empty, full or mixed.
    #[test]
    fn extended_snapshot_is_always_444_bytes() {
        // A 17-byte header (u8 and four u32), then 7 records of u8 index and a 60-byte block.
        let mut mixed = [None; TRADE_SLOT_COUNT];
        mixed[0] = Some(0xABCD);
        mixed[TRADE_SLOT_NONTRADED] = Some(0x1111);
        for entries in [
            [None; TRADE_SLOT_COUNT],
            [Some(0xABCD); TRADE_SLOT_COUNT],
            mixed,
        ] {
            let mut wire = extended_wire(0, 0, 0, entries);
            assert_eq!(wire.len(), 17 + TRADE_SLOT_COUNT * 61);
            assert_eq!(wire.len(), 444);
            assert!(
                read_trade_status_extended(&mut &wire[..443]).is_err(),
                "443 bytes is a short read"
            );
            wire.push(0xEE); // a byte past the snapshot
            let mut r = &wire[..];
            read_trade_status_extended(&mut r).unwrap();
            assert_eq!(r, [0xEE], "the parser stops at byte 444");
        }
    }

    #[test]
    fn extended_parses_header_slots_and_signed_charges() {
        let mut entries = [None; TRADE_SLOT_COUNT];
        entries[0] = Some(0xABCD);
        entries[TRADE_SLOT_NONTRADED] = Some(0x1111); // the enchant slot carries an item too
        let wire = extended_wire(1, 12_345, 777, entries);
        let mut r = &wire[..];
        let ext = read_trade_status_extended(&mut r).unwrap();
        assert!(r.is_empty(), "the whole snapshot must be consumed");

        assert!(ext.their_window);
        assert_eq!(ext.gold, 12_345);
        assert_eq!(ext.enchant_spell_id, 777);

        let slot0 = ext.slots[0].expect("slot 0 filled");
        assert_eq!(slot0.entry, 0xABCD);
        assert_eq!(slot0.display_id, 11);
        assert_eq!(slot0.count, 5);
        assert_eq!(slot0.charges, -3);
        assert_eq!(slot0.max_durability, 100);
        assert_eq!(slot0.durability, 80);

        assert!(
            ext.slots[TRADE_SLOT_NONTRADED].is_some(),
            "enchant slot parsed"
        );
        assert!(ext.slots[1].is_none(), "empty slot folds to None");
        assert!(ext.slots[5].is_none());
    }

    /// The header byte for byte: side, then the sequence, then the slot count, gold and spell. The
    /// sequence is the first `u32` (`0x5d4990` reads it into `[ebp-0xc]` before the others), so a
    /// sender that counts its edits is read as it wrote them, not as the slot count after it.
    #[test]
    fn extended_reads_the_sequence_before_the_slot_count() {
        let mut wire = vec![
            0x00, // side: ours
            0x0B, 0x00, 0x00, 0x00, // sequence 11
            0x07, 0x00, 0x00, 0x00, // slot count 7
            0x39, 0x30, 0x00, 0x00, // gold 12345
            0x09, 0x03, 0x00, 0x00, // spell 777
        ];
        for i in 0..TRADE_SLOT_COUNT {
            wire.push(i as u8);
            wire.extend_from_slice(&[0u8; 60]);
        }
        assert_eq!(wire.len(), 444);
        let ext = read_trade_status_extended(&mut &wire[..]).unwrap();
        assert_eq!(ext.sequence, 11);
        assert_eq!((ext.gold, ext.enchant_spell_id), (12_345, 777));
        // What vmangos sends: the slot count in both words.
        let vmangos = extended_wire(0, 0, 0, [None; TRADE_SLOT_COUNT]);
        assert_eq!(
            read_trade_status_extended(&mut &vmangos[..])
                .unwrap()
                .sequence,
            7
        );
    }

    #[test]
    fn extended_own_window_flag() {
        let wire = extended_wire(0, 0, 0, [None; TRADE_SLOT_COUNT]);
        let mut r = &wire[..];
        let ext = read_trade_status_extended(&mut r).unwrap();
        assert!(!ext.their_window, "which == 0 is our own column");
    }
}
