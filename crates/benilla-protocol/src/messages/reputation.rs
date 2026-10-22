//! The reputation pane's send verbs (vmangos `Misc.cpp`). A faction is addressed by its
//! reputation-list id (`Faction.dbc` `reputationIndex`, its `SMSG_INITIALIZE_FACTIONS` slot),
//! never by the DBC id or the panel row. None is acked, so the client keeps its own copy: the
//! inactive bit returns only at the next login, the at-war bit there or in the server's own
//! `SMSG_SET_FACTION_ATWAR`, and the watched index as a descriptor update.

/// Length of the reputation list `SMSG_INITIALIZE_FACTIONS` is positional in (vmangos `Misc.h:607`,
/// `MAX_FACTION_COUNT`); every `repListId` on this wire indexes it.
pub const FACTION_LIST_LEN: usize = 64;

/// "Watch no faction" for `CMSG_SET_WATCHED_FACTION` and `PLAYER_FIELD_WATCHED_FACTION_INDEX`. Not
/// 0: slot 0 is a real faction (Bloodsail Buccaneers) and vmangos stores the value as sent
/// (`HandleSetWatchedFactionOpcode`). FrameXML's row 0 means none; the binding maps it to this.
pub const WATCHED_FACTION_NONE: i32 = -1;

/// Body of `CMSG_SET_FACTION_ATWAR`. vmangos drops it without a reply while the player is in combat
/// (`HandleSetFactionAtWarOpcode`).
pub fn set_faction_at_war(rep_list_id: u32, at_war: bool) -> Vec<u8> {
    let mut out = rep_list_id.to_le_bytes().to_vec();
    out.push(u8::from(at_war));
    out
}

/// Body of `CMSG_SET_FACTION_INACTIVE`.
pub fn set_faction_inactive(rep_list_id: u32, inactive: bool) -> Vec<u8> {
    let mut out = rep_list_id.to_le_bytes().to_vec();
    out.push(u8::from(inactive));
    out
}

/// Body of `CMSG_SET_WATCHED_FACTION`: a slot, or [`WATCHED_FACTION_NONE`] to stop watching.
pub fn set_watched_faction(rep_list_id: i32) -> Vec<u8> {
    rep_list_id.to_le_bytes().to_vec()
}
