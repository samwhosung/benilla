//! The petition family (opcodes `0x1BB`-`0x1C7`, `0x2C1`): the guild-charter flow. vmangos
//! registers `CMSG_GUILD_CREATE` as `STATUS_NEVER`, so a guild is founded only by buying a
//! charter, collecting nine signatures and turning it in. The charter item's enchantment slot 0
//! holds the petition id (`PetitionsHandler.cpp:126`); the reference tooltip skips enchant lines
//! on a charter (`0x52c9ee`), so nothing may read that slot as an enchantment.

use std::io::{self, Read};

use crate::wire::{
    capacity_hint, read_cstring, read_i32_le, read_u16_le, read_u32_le, read_u64_le, read_u8,
};

/// The guild charter's item entry, vmangos `GUILD_CHARTER` (`PetitionsHandler.cpp:37`).
pub const CHARTER_ITEM_ENTRY: u32 = 5863;

/// The charter's display id, vmangos `CHARTER_DISPLAY_ID` (`PetitionsHandler.cpp:39`).
pub const CHARTER_DISPLAY_ID: u32 = 16161;

/// The template-flag bit of a signable petition (vmangos `ItemPrototype.h:77`); the reference
/// tooltip keys on it for the green `ITEM_SIGNABLE` line and to suppress enchantment lines.
pub const ITEM_FLAG_CHARTER: u32 = 0x0000_2000;

/// The most signatures a charter holds (`PetitionFrame.lua:1`, `PetitionsHandler.cpp:269`); the
/// requirement is [`PetitionQueryResponse::min_signatures`].
pub const MAX_PETITION_SIGNATURES: usize = 9;

/// Charter-name cap in characters (vmangos `MAX_CHARTER_NAME`, `ObjectMgr.h:401`); equal to
/// [`super::GUILD_NAME_MAX_LENGTH`] because the charter name becomes the guild's.
pub const CHARTER_NAME_MAX_LENGTH: usize = 24;

/// The result code shared by `SMSG_PETITION_SIGN_RESULTS` and `SMSG_TURN_IN_PETITION_RESULTS`
/// (vmangos `PetitionSigns`, `Guild/Guild.h:147-154`); each packet uses its own subset.
pub mod petition_result {
    /// Signed, or turned in; both packets.
    pub const OK: u32 = 0;
    /// Sign only: this account already signed. vmangos checks per account before per character
    /// (`GuildMgr.cpp:388-400`), so an alt on the same account gets this.
    pub const ALREADY_SIGNED: u32 = 1;
    /// Turn-in only: the founder is already in a guild (`PetitionsHandler.cpp:427`); a guilded
    /// signer gets `SMSG_GUILD_COMMAND_RESULT` instead (`:258`).
    pub const ALREADY_IN_GUILD: u32 = 2;
    /// Sign only: you cannot sign your own charter (`PetitionsHandler.cpp:237`).
    pub const CANT_SIGN_OWN: u32 = 3;
    /// Turn-in only: not enough signatures (`PetitionsHandler.cpp:438`); vmangos requires exactly
    /// `MinPetitionSigns` (`GuildMgr.h:124`).
    pub const NEED_MORE: u32 = 4;
    /// Not on the same realm (`ERR_PETITION_NOT_SAME_SERVER`); vmangos never sends it.
    pub const NOT_SERVER: u32 = 5;
}

/// One row of `SMSG_PETITION_SHOWLIST`: a charter the NPC sells. vmangos always sends exactly one
/// (`Petition.h:176`: "only 1 element is supported in the client").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionShowListEntry {
    /// Row index, 1-based (`PetitionsHandler.cpp:496`).
    pub index: u32,
    /// The item entry sold, [`CHARTER_ITEM_ENTRY`] in practice.
    pub charter_entry: u32,
    /// Its display id, [`CHARTER_DISPLAY_ID`] in practice.
    pub charter_display_id: u32,
    /// Price in copper, an `int32` on the wire (vmangos charges 1000, `PetitionsHandler.cpp:38`).
    pub charter_cost: i32,
    /// vmangos sends 1; its header says a row "must be `&1` to show it in the UI"
    /// (`Petition.h:169`).
    pub entry_flags: i32,
}

/// `SMSG_PETITION_SHOWLIST`: the registrar NPC's charter list, which opens its window.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionShowList {
    /// The NPC; every later verb in the flow that names an NPC takes this guid.
    pub npc: u64,
    pub entries: Vec<PetitionShowListEntry>,
}

/// Read `SMSG_PETITION_SHOWLIST` (vmangos `Packets/Petition.cpp:115-127`): `u64 npc, u8 count`,
/// then per row `u32 index, u32 entry, u32 displayId, i32 cost, i32 flags`. The request handler
/// sends it only for an NPC in range with `UNIT_NPC_FLAG_PETITIONER` (`PetitionsHandler.cpp:484`).
pub(super) fn read_petition_show_list(r: &mut impl Read) -> io::Result<PetitionShowList> {
    let npc = read_u64_le(r)?;
    let count = read_u8(r)?;
    // vmangos sends exactly one charter (`PetitionsHandler.cpp:504`); 8 is generous.
    let mut entries = Vec::with_capacity(capacity_hint(count, 8));
    for _ in 0..count {
        entries.push(PetitionShowListEntry {
            index: read_u32_le(r)?,
            charter_entry: read_u32_le(r)?,
            charter_display_id: read_u32_le(r)?,
            charter_cost: read_i32_le(r)?,
            entry_flags: read_i32_le(r)?,
        });
    }
    Ok(PetitionShowList { npc, entries })
}

/// One signature on a charter: the signer's guid and a trailing dword.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionSignature {
    /// The signer's guid; the packet carries no names.
    pub signer: u64,
    /// Always 0 from vmangos (`GuildMgr.cpp:358-366`); decoded, not skipped.
    pub unknown: u32,
}

/// `SMSG_PETITION_SHOW_SIGNATURES`: a charter's signers, in answer to our request or when
/// someone offers us theirs. Its name and requirement come from `SMSG_PETITION_QUERY_RESPONSE`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionShowSignatures {
    /// The charter item's guid, which every later verb takes; not the petition id.
    pub item: u64,
    /// The charter's owner; whether it is us decides the leader's view or a signer's.
    pub owner: u64,
    /// The key `CMSG_PETITION_QUERY` takes, also carried in the charter's enchantment slot 0.
    pub petition_id: u32,
    pub signatures: Vec<PetitionSignature>,
}

/// Read `SMSG_PETITION_SHOW_SIGNATURES` (hand-built at vmangos `PetitionsHandler.cpp:160-168`,
/// `:390-397`): `u64 item, u64 owner, u32 petitionId, u8 count`, then `u64 signer, u32 0` each.
pub(super) fn read_petition_show_signatures(
    r: &mut impl Read,
) -> io::Result<PetitionShowSignatures> {
    let item = read_u64_le(r)?;
    let owner = read_u64_le(r)?;
    let petition_id = read_u32_le(r)?;
    let count = read_u8(r)?;
    // "Client hard limit at 9 signatures" (vmangos `PetitionsHandler.cpp:269-270`).
    let mut signatures = Vec::with_capacity(capacity_hint(count, 9));
    for _ in 0..count {
        signatures.push(PetitionSignature {
            signer: read_u64_le(r)?,
            unknown: read_u32_le(r)?,
        });
    }
    Ok(PetitionShowSignatures {
        item,
        owner,
        petition_id,
        signatures,
    })
}

/// `SMSG_PETITION_SIGN_RESULTS`: the verdict on one signature. A success goes, identical, to the
/// signer and the online owner (`PetitionsHandler.cpp:299`, `:312-316`); both name the signer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionSignResults {
    /// The charter item's guid.
    pub item: u64,
    /// The signer's guid, in both copies.
    pub player: u64,
    /// A [`petition_result`] code.
    pub result: u32,
}

/// Read `SMSG_PETITION_SIGN_RESULTS` (vmangos `Packets/Petition.cpp:69-74`).
pub(super) fn read_petition_sign_results(r: &mut impl Read) -> io::Result<PetitionSignResults> {
    Ok(PetitionSignResults {
        item: read_u64_le(r)?,
        player: read_u64_le(r)?,
        result: read_u32_le(r)?,
    })
}

/// `SMSG_PETITION_QUERY_RESPONSE`: a petition's record, keyed by petition id.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionQueryResponse {
    /// The petition id we asked about.
    pub petition_id: u32,
    /// The charter owner's character guid.
    pub owner: u64,
    /// The proposed guild's name, carried by no other packet.
    pub name: String,
    /// Free text on the charter; empty, as vmangos never sets it (`PetitionsHandler.cpp:177-183`).
    pub body_text: String,
    /// vmangos sends 1 (`PetitionsHandler.cpp:181`); meaning unknown.
    pub flags: u32,
    /// Signatures required, which the reference's Request-Signature button tests; vmangos sends 9
    /// even when its `MinPetitionSigns` is lower (`PetitionsHandler.cpp:182`).
    pub min_signatures: u32,
    /// Signatures that fit; 9 on vmangos (`PetitionsHandler.cpp:183`).
    pub max_signatures: u32,
    /// Deadline timestamp, 0 on vmangos.
    pub deadline: u32,
    /// Creation timestamp, 0 on vmangos.
    pub creation: u32,
    /// Restricting guild id, 0 on vmangos.
    pub allowed_guild_id: u32,
    /// Class mask restriction, 0 on vmangos.
    pub allowed_classes: u32,
    /// Race mask restriction, 0 on vmangos.
    pub allowed_races: u32,
    /// Gender restriction: a `u16`, the packet's one odd width (`Packets/Petition.cpp:95`).
    pub allowed_gender: u16,
    /// Minimum level, 0 on vmangos.
    pub allowed_min_level: u32,
    /// Maximum level, 0 on vmangos.
    pub allowed_max_level: u32,
    /// The petition's multiple-choice options; empty on vmangos.
    pub choices: Vec<String>,
    /// The pre-selected choice, 0 on vmangos.
    pub default_choice: u32,
}

/// Read `SMSG_PETITION_QUERY_RESPONSE` (vmangos `Packets/Petition.cpp:81-102`): the fields in
/// struct order, the choices as a `u32` count of C strings.
pub(super) fn read_petition_query_response(r: &mut impl Read) -> io::Result<PetitionQueryResponse> {
    let petition_id = read_u32_le(r)?;
    let owner = read_u64_le(r)?;
    let name = read_cstring(r)?;
    let body_text = read_cstring(r)?;
    let flags = read_u32_le(r)?;
    let min_signatures = read_u32_le(r)?;
    let max_signatures = read_u32_le(r)?;
    let deadline = read_u32_le(r)?;
    let creation = read_u32_le(r)?;
    let allowed_guild_id = read_u32_le(r)?;
    let allowed_classes = read_u32_le(r)?;
    let allowed_races = read_u32_le(r)?;
    let allowed_gender = read_u16_le(r)?;
    let allowed_min_level = read_u32_le(r)?;
    let allowed_max_level = read_u32_le(r)?;
    let choice_count = read_u32_le(r)?;
    let mut choices = Vec::with_capacity(capacity_hint(choice_count, 64));
    for _ in 0..choice_count {
        choices.push(read_cstring(r)?);
    }
    Ok(PetitionQueryResponse {
        petition_id,
        owner,
        name,
        body_text,
        flags,
        min_signatures,
        max_signatures,
        deadline,
        creation,
        allowed_guild_id,
        allowed_classes,
        allowed_races,
        allowed_gender,
        allowed_min_level,
        allowed_max_level,
        choices,
        default_choice: read_u32_le(r)?,
    })
}

/// `MSG_PETITION_RENAME`: the server's echo of a successful rename (`PetitionsHandler.cpp:209`);
/// a rejected name gets `SMSG_GUILD_COMMAND_RESULT`. Any holder of the item may rename it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionRename {
    /// The charter item's guid.
    pub item: u64,
    pub name: String,
}

/// Read `MSG_PETITION_RENAME` (vmangos `Packets/Petition.cpp:104-108`): `u64` item, cstring name.
pub(super) fn read_petition_rename(r: &mut impl Read) -> io::Result<PetitionRename> {
    Ok(PetitionRename {
        item: read_u64_le(r)?,
        name: read_cstring(r)?,
    })
}

/// Read `SMSG_TURN_IN_PETITION_RESULTS` (vmangos `Packets/Petition.cpp:76-79`): a lone
/// [`petition_result`] code. A name collision sends `SMSG_GUILD_COMMAND_RESULT` and no result
/// packet at all (`PetitionsHandler.cpp:445`).
pub(super) fn read_turn_in_petition_results(r: &mut impl Read) -> io::Result<u32> {
    read_u32_le(r)
}

/// Read `MSG_PETITION_DECLINE` inbound (vmangos `Packets/Petition.cpp:110-113`): the decliner's
/// guid, sent to the owner only. The outbound form carries the item guid instead.
pub(super) fn read_petition_decline(r: &mut impl Read) -> io::Result<u64> {
    read_u64_le(r)
}

/// Body of `CMSG_PETITION_SHOWLIST` (vmangos `Packets/Petition.cpp:3-6`): the NPC's guid. The
/// server also sends the list unasked for a `GOSSIP_OPTION_PETITIONER` row (`Player.cpp:12428`).
pub fn petition_show_list(npc: u64) -> Vec<u8> {
    npc.to_le_bytes().to_vec()
}

/// Body of `CMSG_PETITION_BUY` (vmangos `Packets/Petition.cpp:47-67`): `u64 npc, u32 0, u64 0`,
/// the name, `u32 0` x10, `u16 0, u8 0, u32 index, u32 0`. The server reads only NPC and name.
/// A bad name gets `SMSG_GUILD_COMMAND_RESULT`, no money `SMSG_BUY_FAILED`, a full bag
/// `SMSG_INVENTORY_CHANGE_FAILURE`; success is only the item push (`PetitionsHandler.cpp:78-130`).
pub fn petition_buy(npc: u64, name: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(72 + name.len());
    body.extend_from_slice(&npc.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&0u64.to_le_bytes());
    push_cstring(&mut body, name);
    body.extend_from_slice(&[0u8; 40]); // 10 × u32
    body.extend_from_slice(&0u16.to_le_bytes());
    body.push(0);
    body.extend_from_slice(&0u32.to_le_bytes()); // index, "unused" per the server
    body.extend_from_slice(&0u32.to_le_bytes());
    body
}

/// Body of `CMSG_PETITION_SHOW_SIGNATURES` (vmangos `Packets/Petition.cpp:8-11`): the charter
/// item's guid, unanswered if we are guilded or lack the item (`PetitionsHandler.cpp:140-143`).
pub fn petition_show_signatures(item: u64) -> Vec<u8> {
    item.to_le_bytes().to_vec()
}

/// Body of `CMSG_PETITION_SIGN` (vmangos `Packets/Petition.cpp:35-39`): the item guid, then an
/// `i8` the server skips. `SignPetition` defaults it to 1 (`0x4f46d9`, sent at `0x4f4749`).
pub fn petition_sign(item: u64, arg: i8) -> Vec<u8> {
    let mut body = Vec::with_capacity(9);
    body.extend_from_slice(&item.to_le_bytes());
    body.push(arg as u8);
    body
}

/// Body of `CMSG_OFFER_PETITION` (vmangos `Packets/Petition.cpp:41-45`): item guid, target guid.
/// Success sends the target `SMSG_PETITION_SHOW_SIGNATURES` (`PetitionsHandler.cpp:390-397`); we
/// hear only refusals, as `SMSG_GUILD_COMMAND_RESULT`.
pub fn offer_petition(item: u64, player: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(16);
    body.extend_from_slice(&item.to_le_bytes());
    body.extend_from_slice(&player.to_le_bytes());
    body
}

/// Body of `CMSG_TURN_IN_PETITION` (vmangos `Packets/Petition.cpp:24-27`): the charter item's
/// guid. Only the owner may turn it in; anyone else gets silence (`PetitionsHandler.cpp:432`).
/// Success founds the guild with every signer at the lowest rank.
pub fn turn_in_petition(item: u64) -> Vec<u8> {
    item.to_le_bytes().to_vec()
}

/// Body of `CMSG_PETITION_QUERY` (vmangos `Packets/Petition.cpp:13-17`): `u32 petitionId`,
/// `u64 item`; the server looks up by id alone (`PetitionsHandler.cpp:171-185`).
pub fn petition_query(petition_id: u32, item: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&petition_id.to_le_bytes());
    body.extend_from_slice(&item.to_le_bytes());
    body
}

/// Body of `MSG_PETITION_RENAME` outbound (vmangos `Packets/Petition.cpp:29-33`): `u64` item,
/// cstring name. Not truncated to [`CHARTER_NAME_MAX_LENGTH`]; the server validates it.
pub fn petition_rename(item: u64, name: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(9 + name.len());
    body.extend_from_slice(&item.to_le_bytes());
    push_cstring(&mut body, name);
    body
}

/// Body of `MSG_PETITION_DECLINE` outbound (vmangos `Packets/Petition.cpp:19-22`): the charter
/// item's guid; the server forwards our guid to the owner.
pub fn petition_decline(item: u64) -> Vec<u8> {
    item.to_le_bytes().to_vec()
}

fn push_cstring(body: &mut Vec<u8>, s: &str) {
    body.extend_from_slice(s.as_bytes());
    body.push(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A body one byte short leaves vmangos's reader mid-field and the buy is silently dropped.
    #[test]
    fn petition_buy_body_is_seventy_two_bytes_plus_the_name() {
        let body = petition_buy(0x1234_5678_9abc_def0, "Test");
        // 8 npc + 4 + 8 + (4+1) name + 40 + 2 + 1 + 4 + 4
        assert_eq!(body.len(), 72 + 4, "72 fixed bytes + a 4-char name");
        assert_eq!(
            petition_buy(0, "").len(),
            72,
            "the empty name still costs its NUL"
        );
    }

    #[test]
    fn charter_constants_match_the_world_data() {
        assert_eq!(CHARTER_ITEM_ENTRY, 5863);
        assert_eq!(CHARTER_DISPLAY_ID, 16161);
        assert_eq!(ITEM_FLAG_CHARTER, 0x2000);
        assert_eq!(CHARTER_NAME_MAX_LENGTH, super::super::GUILD_NAME_MAX_LENGTH);
    }
}
