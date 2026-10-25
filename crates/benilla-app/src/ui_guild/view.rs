//! What the guild packets write that the getters read, as values a lane's item can hold: the
//! roster message's state replaced whole by each message, and the guild records by id. Both sit
//! behind an `Arc`, so a version of the state is a pointer clone and a write after one copies
//! only what it touches ([`Arc::make_mut`]). In the reference each handler writes the roster
//! module's cells (`0xb73118`, `0xb72704`, `0xb726d0`, `0xb72f00`) and signals inside itself
//! (`0x4d0ad0`, `0x5e7180`, `0x5e7520`), so a listener reads its own packet's cells; the guild
//! lane fires each packet's events with the version that packet left ([`super::lane`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use benilla_protocol::messages::GuildRosterMember;

use super::Identity;

/// The next member list's number: it keys the display order cached for that list.
static NEXT_MEMBERS: AtomicU64 = AtomicU64::new(1);

/// The roster message's state (`0x4d0ad0`): the message of the day, the information text, the rank
/// rights and the members in wire order, replaced whole by each roster message.
#[derive(Clone, Debug, Default)]
pub(crate) struct Roster {
    pub(super) motd: String,
    pub(super) info_text: String,
    /// One rights word per rank id; its length is the guild's real rank count.
    pub(super) rank_rights: Vec<u32>,
    pub(super) members: Vec<GuildRosterMember>,
    /// The members on line, tallied as the message lands (`0xb7311c`).
    pub(super) online: usize,
    /// Which member list this is, new with each list.
    pub(super) members_rev: u64,
}

impl Roster {
    /// A roster of `members` with the rest as it stands in `self`, a new list.
    pub(super) fn set_members(&mut self, members: Vec<GuildRosterMember>) {
        self.online = members.iter().filter(|m| m.is_online()).count();
        self.members = members;
        self.members_rev = NEXT_MEMBERS.fetch_add(1, Ordering::Relaxed);
    }

    /// The rights word of rank `index`, 0 at the array's end and past it (`0x4d18f2`).
    pub(super) fn rights(&self, index: u32) -> u32 {
        self.rank_rights.get(index as usize).copied().unwrap_or(0)
    }
}

/// The guild state a getter reads as of one place: the roster and the guild records by id.
#[derive(Clone, Debug, Default)]
pub(crate) struct GuildView {
    pub(super) roster: Arc<Roster>,
    pub(super) identities: Arc<HashMap<u32, Arc<Identity>>>,
}

impl GuildView {
    /// Whether `other` is this very version: nothing was written since.
    pub(super) fn same(&self, other: &GuildView) -> bool {
        Arc::ptr_eq(&self.roster, &other.roster) && Arc::ptr_eq(&self.identities, &other.identities)
    }
}
