//! Gossip and NPC text, the dialog a friendly NPC opens (vmangos `Npc.cpp`, `GossipDef.cpp`).

use std::io;

use crate::wire::{capacity_hint, read_cstring, read_f32_le, read_u32_le, read_u64_le, read_u8};

/// One `SMSG_GOSSIP_MESSAGE` option: `index` is echoed as `gossipListId` on select, `icon` is a
/// `GOSSIP_ICON_*` (0 chat, 1 vendor, 2 taxi, 3 trainer, …), `coded` asks for a code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GossipOption {
    pub index: u32,
    pub icon: u8,
    pub coded: bool,
    pub message: String,
}

/// One quest in `SMSG_GOSSIP_MESSAGE`'s quest list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestOption {
    pub quest_id: u32,
    pub icon: u32,
    pub level: u32,
    pub title: String,
}

/// `CMSG_GOSSIP_HELLO` body: the NPC's full guid (`Npc.cpp:3`); it works on any interactable
/// creature, gossip flag or not (`NPCHandler.cpp:347`).
pub fn gossip_hello(npc_guid: u64) -> Vec<u8> {
    npc_guid.to_le_bytes().to_vec()
}

/// `CMSG_GOSSIP_SELECT_OPTION` body: guid, the option's `index`, then the code only for a coded
/// option; the server reads a code whenever bytes remain, so omit it otherwise (`Npc.cpp:78-86`).
pub fn gossip_select_option(npc_guid: u64, gossip_list_id: u32, code: Option<&str>) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&npc_guid.to_le_bytes());
    body.extend_from_slice(&gossip_list_id.to_le_bytes());
    if let Some(code) = code {
        body.extend_from_slice(code.as_bytes());
        body.push(0);
    }
    body
}

/// `CMSG_NPC_TEXT_QUERY` body: the text id, then the NPC's guid (`Npc.cpp:8-12`).
pub fn npc_text_query(text_id: u32, guid: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&text_id.to_le_bytes());
    body.extend_from_slice(&guid.to_le_bytes());
    body
}

/// Read `SMSG_GOSSIP_MESSAGE` (vmangos `GossipDef.cpp:180-225`); 1.12 options have no box money.
pub(super) fn read_gossip_message(
    r: &mut &[u8],
) -> io::Result<(u64, u32, Vec<GossipOption>, Vec<QuestOption>)> {
    let npc_guid = read_u64_le(r)?;
    let text_id = read_u32_le(r)?;
    let option_count = read_u32_le(r)?;
    // vmangos `GOSSIP_MAX_MENU_ITEMS` 32 (`GossipDef.h:32`, "client supports showing max 32").
    let mut options = Vec::with_capacity(capacity_hint(option_count, 32));
    for _ in 0..option_count {
        options.push(GossipOption {
            index: read_u32_le(r)?,
            icon: read_u8(r)?,
            coded: read_u8(r)? != 0,
            message: read_cstring(r)?,
        });
    }
    let quest_count = read_u32_le(r)?;
    // Unbounded server-side (`GossipDef.h:184`); the 32-item display limit bounds the hint.
    let mut quests = Vec::with_capacity(capacity_hint(quest_count, 32));
    for _ in 0..quest_count {
        quests.push(QuestOption {
            quest_id: read_u32_le(r)?,
            icon: read_u32_le(r)?,
            level: read_u32_le(r)?,
            title: read_cstring(r)?,
        });
    }
    Ok((npc_guid, text_id, options, quests))
}

/// One of the eight greeting variants of an `SMSG_NPC_TEXT_UPDATE` record, both gender columns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NpcTextBlock {
    /// The block's draw weight, not a rank; vmangos's fallback record sets all eight to 0.
    pub probability: f32,
    /// `text0`, shown when the NPC is male or genderless.
    pub male: String,
    /// `text1`, shown when the NPC is female.
    pub female: String,
}

impl NpcTextBlock {
    fn column(&self, female: bool) -> &str {
        if female {
            &self.female
        } else {
            &self.male
        }
    }
}

/// The greeting blocks in every `SMSG_NPC_TEXT_UPDATE` record.
pub const NPC_TEXT_BLOCKS: usize = 8;

/// Read `SMSG_NPC_TEXT_UPDATE` (vmangos `GossipDef.cpp:298-369`), all eight blocks undecided; the
/// greeting is picked when the frame opens, by [`select_greeting`].
///
/// The language is parsed and dropped: the 1.12 client garbles a block in a non-zero language
/// (`0x49b560`), which is not built, and no 1.12 record is known to set one.
pub(super) fn read_npc_text_update(r: &mut &[u8]) -> io::Result<(u32, Vec<NpcTextBlock>)> {
    let text_id = read_u32_le(r)?;
    let mut blocks = Vec::with_capacity(NPC_TEXT_BLOCKS);
    for _ in 0..NPC_TEXT_BLOCKS {
        let probability = read_f32_le(r)?;
        let male = read_cstring(r)?;
        let female = read_cstring(r)?;
        let _language_id = read_u32_le(r)?;
        for _ in 0..3 {
            let _emote_delay = read_u32_le(r)?;
            let _emote_id = read_u32_le(r)?;
        }
        blocks.push(NpcTextBlock {
            probability,
            male,
            female,
        });
    }
    Ok((text_id, blocks))
}

/// `SMSG_GOSSIP_POI`, the map flag a guard drops when asked for directions; the server sends it
/// unasked from a gossip option (`Player::OnGossipSelect`). The 1.12 client treats it as an
/// `AreaPOI.dbc` record in minimap blip slot 1 (`0x4e2840`), hence the DBC field names.
#[derive(Debug, Clone, PartialEq)]
pub struct GossipPoi {
    /// As `AreaPOI.dbc` `Flags`: bit 0 makes it a candidate, bit 1 draws the in-range icon; every
    /// vmangos row sends 99.
    pub flags: u32,
    /// World x and y; the wire carries no z.
    pub pos: [f32; 2],
    /// The `POIIcons.blp` cell (8×8 grid, drawn only below 64); every vmangos row uses 6, the red
    /// flag (`GossipDef.h:113`).
    pub icon: u32,
    /// vmangos `points_of_interest.data`, 0 in every row.
    pub data: u32,
    /// The destination's name, the marker's tooltip.
    pub name: String,
}

/// Read `SMSG_GOSSIP_POI` (vmangos `GossipDef.cpp:239-295`).
pub(super) fn read_gossip_poi(r: &mut &[u8]) -> io::Result<GossipPoi> {
    let flags = read_u32_le(r)?;
    let x = read_f32_le(r)?;
    let y = read_f32_le(r)?;
    Ok(GossipPoi {
        flags,
        pos: [x, y],
        icon: read_u32_le(r)?,
        data: read_u32_le(r)?,
        name: read_cstring(r)?,
    })
}

/// Pick the greeting as the 1.12 client does (`0x4e2010`): a weighted draw over the blocks with
/// text in the NPC's gender column (female only at 1), never falling back to the other column.
/// `roll` is uniform in `[1.0, 2.0)`, `<=` makes an all-zero record pick block 0, and the sums
/// are `f64` as on the client's x87 stack. `None` is its "Missing gossip text!".
pub fn select_greeting(blocks: &[NpcTextBlock], npc_gender: u8, roll: f32) -> Option<&str> {
    let female = npc_gender == 1;
    let drawn = || blocks.iter().filter(|b| !b.column(female).is_empty());
    let sum: f64 = drawn().map(|b| b.probability as f64).sum();
    let threshold = (2.0 - roll as f64) * sum;
    let mut acc = 0.0f64;
    for block in drawn() {
        acc += block.probability as f64;
        if threshold <= acc {
            return Some(block.column(female));
        }
    }
    None
}
