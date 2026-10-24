//! The guild session: the identity cache, the roster, the ranks and the outbound verbs.
//! [`GuildState`] mirrors the guild packets and [`feed`] turns it into the snapshot the guild
//! windows read. The roster carries no guild or rank names: those come only from
//! `SMSG_GUILD_QUERY_RESPONSE`, cached by guild id and filled lazily, as in the reference.

use std::collections::HashMap;
use std::sync::Mutex;

use benilla_formats::GuildEmblem;
use benilla_protocol::messages::{
    guild_event, GuildCommandResult, GuildEventNotice, GuildInfo, GuildQueryResponse, GuildRoster,
    GuildRosterMember, GUILD_RANKS_MAX_COUNT,
};
use benilla_protocol::ObjectFields;
use benilla_ui::script::{LastOnline, UiScript, UnitGuild};
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::{Ask, QueryCache, Refused};
use crate::ui_script::UiFeed;

#[cfg(test)]
mod call_tests;
mod feed;
mod lines;
mod sort;
mod steps;

pub(crate) use sort::RosterRow;
use sort::{SortField, SortStack};
pub(crate) use steps::GuildSteps;

/// `GuildRoster()`'s silence after a request: the reference's 10 000 ms against `0xb73130`
/// (`0x4d10d0`). The arg1 edge of [`RosterUpdate`] clears it.
const ROSTER_REQUEST_THROTTLE_SECS: f64 = 10.0;

/// One guild's identity, from `SMSG_GUILD_QUERY_RESPONSE`.
#[derive(Clone, Debug, Default)]
struct Identity {
    /// Empty means no such guild, as the reference reads it (`0x5552ae`: cache insert `0x561070`
    /// or remove `0x561390`); cached as a negative so the query is not re-sent.
    name: String,
    /// The ten rank names, index 0 = guild master; empty past the guild's real rank count.
    rank_names: [String; GUILD_RANKS_MAX_COUNT],
    /// The tabard's five emblem indices for the body composite: the reference reads this record
    /// from its character compositor (`0x6d6d20`, `0x47a610`) as well as from `GetGuildInfo`.
    emblem: GuildEmblem,
}

impl Identity {
    /// The name of rank `index`, or `""`. Deviation: bounded at the ten slots, because the
    /// reference's read (`0x4c93e7`) runs past its array.
    fn rank_name(&self, index: u32) -> &str {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.rank_names.get(i))
            .map(String::as_str)
            .unwrap_or_default()
    }
}

/// Which `GUILD_ROSTER_UPDATE` the feed owes. A truthy arg1 makes the stock pane re-request the
/// roster (`FriendsFrame.lua:646-653`). The reference's `0x4d1160` fires it bare for a parsed
/// roster, a show-offline change, a re-sort and a landed guild record (`0x4d0d3c`, `0x4d0f9f`,
/// `0x4d1022`, `0x4d148c`), and with arg1 only for `SMSG_GUILD_EVENT` cases `0x00`-`0x0b`
/// (`0x5e74c4`) and two `SMSG_GUILD_COMMAND_RESULT` arms (`0x5e7792`), clearing the
/// `GuildRoster()` throttle first. arg1 on a parsed roster would re-request forever.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RosterUpdate {
    /// Repaint: a roster just applied, a local re-order, a late identity.
    Applied,
    /// The roster is stale: re-request it.
    Stale,
}

impl RosterUpdate {
    /// Whether this edge carries arg1.
    fn arg1(self) -> bool {
        self == RosterUpdate::Stale
    }
}

/// The `guildMemberNotify` CVar, Guild Member Alert on the Chat options page (help string at
/// `0x860320`). Off by default: its register site `0x5e24c7` pushes `"0"` (`0x82e570`). Kept apart
/// from [`GuildState`], which is cleared on disconnect.
#[derive(Resource, Default)]
pub(crate) struct GuildMemberNotify(pub(crate) bool);

/// A callback a guild lookup leaves on a record not yet held, added at the list's tail
/// (`0x56100b`): the landing runs each with status 1 (`0x556f91`); a refusal, or a drop of the
/// pending record, runs them with status 0, which both of these ignore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Waiter {
    /// `GetGuildInfo`'s `0x4c9470` for the unit `guid` (`0x4c93b6`-`0x4c93d2`):
    /// `PLAYER_GUILD_UPDATE` through `0x515e50` (`0x4c947c`-`0x4c9481`).
    Info(u64),
    /// The `PLAYER_GUILD_TIMESTAMP` watcher's `0x5e29c0` (`0x5e299d`): `RefreshGuildTag` on every
    /// held player of the guild (`0x5e29e0`).
    Members,
}

/// The guild session mirror: filled by the packet handlers, read by the feed, reset on disconnect,
/// and its roster and ranks at each world entry ([`seed_vm_guild`]).
#[derive(Resource, Default)]
pub(crate) struct GuildState {
    /// Our `PLAYER_GUILDID` (field 191), `0` when guildless; `IsInGuild` reads it, not the roster.
    guild_id: u32,
    /// Our `PLAYER_GUILDRANK` (field 192), 0-based, `0` = guild master.
    rank_index: u32,
    /// Guild id to identity, asked once while in flight; an empty name is a refusal (`0x561390`).
    /// Keyed by id because `PLAYER_GUILDID` is public, so any visible player's guild is nameable
    /// ([`unit_guild`]).
    identities: QueryCache<u32, Identity>,
    /// Bumped when an identity lands or is evicted, never by an ask; the unit feeds watch it.
    identity_generation: u64,
    /// The callbacks waiting on each guild's record, in the order they were left; locked so a
    /// getter's lookup leaves one through `&self`, inside its call ([`Self::wait_for_info`]).
    waiters: Mutex<HashMap<u32, Vec<Waiter>>>,
    /// `0x5e2890`'s table of each guild's last `PLAYER_GUILD_TIMESTAMP` (`0xc4d668`, guild and
    /// stamp), freed when the world goes (`0x5e2e60`).
    stamps: Vec<(u32, u32)>,
    /// Apart from the roster: `GE_MOTD` sets it alone, and at login vmangos sends that event before
    /// any roster (`CharacterHandler.cpp:558`).
    motd: String,
    /// The guild information text (`GuildInfoFrame`), carried only by the roster.
    info_text: String,
    /// One rights word per rank id; its length is the guild's real rank count.
    rank_rights: Vec<u32>,
    /// The members, in wire order.
    members: Vec<GuildRosterMember>,
    roster_event: Option<RosterUpdate>,
    /// The pushed snapshot is stale; the feed rebuilds only when set.
    dirty: bool,
    /// `/ginfo` answers for [`feed`], which builds their lines where the VM is: their templates are
    /// not catalog rows (`0x5e6fb0`), so they cannot ride [`crate::ui_action::UiErrorKeys`].
    pending_info: Vec<GuildInfo>,
}

/// What the guild verbs write inside their calls ([`GuildSteps`]), apart from [`GuildState`] so
/// the unit and item feeds that read the guild cache beside the VM can share the call host: the
/// sort chain, the shown order, the request deadline and the held invitation. Reset with the
/// session, as [`GuildState`] is, and the sort and the deadline at each world entry
/// ([`seed_vm_guild`]). The show-offline flag, the selection and the staged rank are the VM's
/// ([`benilla_ui::script::GuildCells`]).
#[derive(Resource, Default)]
pub(crate) struct GuildLocal {
    sort: SortStack,
    /// Member guids in the order last shown, mapping a Lua row index back to its player.
    display_order: Vec<u64>,
    /// Real-time seconds before which `GuildRoster()` is swallowed (`0xb73130`); `0.0` = allowed.
    roster_allowed_at: f64,
    /// The held invitation, `(inviter, guild)`; the accept and decline packets do not name it.
    pending_invite: Option<(String, String)>,
}

impl GuildLocal {
    /// `SortGuildRoster(field)`: the chain's step (`0x4d0fb0`); the caller re-sorts and signals.
    fn sort_by(&mut self, field: &str) {
        self.sort.select(SortField::parse(field));
    }

    /// The guid at a 1-based display row, over the whole roster, not the show-offline count.
    fn guid_at(&self, index: u32) -> Option<u64> {
        index
            .checked_sub(1)
            .and_then(|i| self.display_order.get(i as usize))
            .copied()
    }

    /// The held invitation, for a test elsewhere.
    #[cfg(test)]
    pub(crate) fn invite(&self) -> Option<&(String, String)> {
        self.pending_invite.as_ref()
    }
}

/// The roster's VM cells between a dying VM and the next ([`fold_dying_vm_guild`]).
#[derive(Resource, Default)]
struct GuildCellsCarry(benilla_ui::script::GuildCells);

/// Take the roster's show-offline flag, selection and staged rank out of the dying VM, which a
/// `/reload`'s VM gets back ([`seed_vm_guild`]).
pub(crate) fn fold_dying_vm_guild(world: &mut World) {
    let Some(cells) = world
        .get_non_send_resource_mut::<UiScript>()
        .map(|mut s| s.take_guild_cells())
    else {
        return;
    };
    world.insert_resource(GuildCellsCarry(cells));
}

/// Seat a new in-game VM's roster state before its load runs. A `/reload` runs only the teardown
/// and `UI_Init` (`0x495664`, `0x495669`), so its VM keeps the last one's cells. A world entry runs
/// `InitGuildRoster` from `InitializeGame` (`0x48f5ea`), which zeroes the show-offline flag, the
/// selection and the request deadline and unstages the rank (`0x4d0a25`-`0x4d0a41`), the member, online and rank counts
/// (`0x4d0a13`-`0x4d0a1f`), and seeds the sort chain afresh (`0x4d0a50`). The message of the day
/// and the info text it leaves.
pub(crate) fn seed_vm_guild(world: &mut World, script: &mut UiScript, reload: bool) {
    let carried = world
        .remove_resource::<GuildCellsCarry>()
        .unwrap_or_default();
    if reload {
        script.seed_guild_cells(carried.0);
        return;
    }
    if let Some(mut local) = world.get_resource_mut::<GuildLocal>() {
        local.sort = SortStack::default();
        local.roster_allowed_at = 0.0;
    }
    if let Some(mut guild) = world.get_resource_mut::<GuildState>() {
        guild.members.clear();
        guild.rank_rights.clear();
        guild.dirty = true;
    }
}

impl crate::query_cache::AskOnce for GuildState {
    fn clear_pending(&mut self) {
        self.identities.clear_pending();
    }
}

impl GuildState {
    /// The world entry's clear ([`crate::wdb`]): the guild store is built with persistence off
    /// (`0x554d20`), so `0x555740` clears it (`0x5557c0`), the callbacks of its pending records
    /// with it.
    pub(crate) fn clear_world_session(&mut self) {
        self.identities.clear();
        self.waiters().clear();
        self.identity_generation = self.identity_generation.wrapping_add(1);
    }

    /// `IsInGuild`: our own `PLAYER_GUILDID` is set, known before any guild packet.
    pub(crate) fn in_guild(&self) -> bool {
        self.guild_id != 0
    }

    /// Whether the mirror holds `guild_id` and `rank_index` already.
    fn mirrors(&self, guild_id: u32, rank_index: u32) -> bool {
        (self.guild_id, self.rank_index) == (guild_id, rank_index)
    }

    /// Mirror our descriptor's guild fields; `true` when either moved. Asks for nothing: the
    /// identity cache is lazy and the roster is the FrameXML's to request.
    fn mirror_self(&mut self, guild_id: u32, rank_index: u32) -> bool {
        if self.mirrors(guild_id, rank_index) {
            return false;
        }
        // A first 0 to N is learning our guild, not changing it: at login the MOTD event arrives
        // before our descriptor (`CharacterHandler.cpp:558`), and a wipe here would lose it. Only
        // a move out of a real guild clears the mirror.
        let known = self.guild_id != 0;
        let moved = self.guild_id != guild_id;
        self.guild_id = guild_id;
        self.rank_index = rank_index;
        if moved && known {
            // The old guild's roster goes; the identity cache is keyed by id and stays true.
            self.motd.clear();
            self.info_text.clear();
            self.rank_rights.clear();
            self.members.clear();
        }
        if moved {
            self.note_roster_update(RosterUpdate::Applied);
        }
        self.dirty = true;
        true
    }

    /// Owe the VM a `GUILD_ROSTER_UPDATE`. [`RosterUpdate::Stale`] wins over `Applied` in one
    /// frame, since a re-request also repaints; its signal clears the request throttle.
    fn note_roster_update(&mut self, kind: RosterUpdate) {
        if kind.arg1() || self.roster_event.is_none() {
            self.roster_event = Some(kind);
        }
        self.dirty = true;
    }

    /// Reads `guild_id` as `ask` says, sending `CMSG_GUILD_QUERY` on an ask. Guild 0 is no guild,
    /// never asked.
    fn resolve_identity(&self, guild_id: u32, ask: Ask<'_>) -> Option<&Identity> {
        self.identities.read(guild_id, ask, |commands| {
            let _ = commands.0.send(ClientCommand::GuildQuery { guild_id });
        })
    }

    /// Asks for `guild_id` and lands the server's refusal of it, for a test elsewhere.
    #[cfg(test)]
    pub(crate) fn refuse_for_test(&mut self, guild_id: u32, commands: &NetCommands) {
        self.resolve_identity(guild_id, Ask::New(commands));
        self.apply_query_response(GuildQueryResponse {
            guild_id,
            name: String::new(),
            ..Default::default()
        });
    }

    /// Lands `guild_id`'s record named `name`, for a test elsewhere.
    #[cfg(test)]
    pub(crate) fn land_for_test(&mut self, guild_id: u32, name: &str) {
        self.apply_query_response(GuildQueryResponse {
            guild_id,
            name: name.to_string(),
            ..Default::default()
        });
    }

    /// Mirrors our descriptor's guild id, as the feed does, for a test elsewhere.
    #[cfg(test)]
    pub(crate) fn join_for_test(&mut self, guild_id: u32) {
        self.mirror_self(guild_id, 0);
    }

    /// Forgets the in-flight asks, as a world entry does, for a test elsewhere.
    #[cfg(test)]
    pub(crate) fn drop_pending_for_test(&mut self) {
        self.identities.clear_pending();
    }

    /// Whether the cache holds an identity, a pending ask or a refusal for `guild_id`; guild 0
    /// is no guild, and so known.
    pub(crate) fn identity_known(&self, guild_id: u32) -> bool {
        guild_id == 0 || self.identities.knows(guild_id)
    }

    /// The landed-identity counter the unit feeds watch.
    pub(crate) fn identity_generation(&self) -> u64 {
        self.identity_generation
    }

    /// Our guild's five tabard fields for the tabard designer, `-1`s when undesigned as the wire
    /// carries them. Only read, as the designer's seed and its save test read it (`0x502921`,
    /// `0x5f0856`).
    pub(crate) fn own_emblem_record(&self, guild_id: u32) -> Option<[i32; 5]> {
        let e = self.resolve_identity(guild_id, Ask::Read)?.emblem;
        Some([
            e.emblem_style,
            e.emblem_color,
            e.border_style,
            e.border_color,
            e.background_color,
        ])
    }

    /// Drop our guild's cached record after a saved emblem (`0x5e715f`), so the next read
    /// re-fetches it and every member in sight re-dresses, as in the reference.
    pub(crate) fn evict_own_identity(&mut self) {
        self.drop_record(self.guild_id);
    }

    /// `0x561d20`: the record goes, and a pending one's callbacks run with status 0, which
    /// signals nothing.
    fn drop_record(&mut self, guild_id: u32) {
        self.waiters().remove(&guild_id);
        if self.identities.evict(guild_id) {
            self.identity_generation = self.identity_generation.wrapping_add(1);
            self.dirty = true;
        }
    }

    fn waiters(&self) -> std::sync::MutexGuard<'_, HashMap<u32, Vec<Waiter>>> {
        self.waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `GetGuildInfo` missed `guild_id` for the unit `guid`: the lookup with its callback
    /// (`0x4c93d2`), asking a key not in flight, a refused one again, and leaving a waiter, with
    /// no dedupe (`push 0`, `0x4c93b6`). A record already held takes no waiter.
    pub(crate) fn wait_for_info(&self, guild_id: u32, guid: u64, commands: &NetCommands) {
        if guild_id != 0
            && self
                .resolve_identity(guild_id, Ask::Event(commands))
                .is_none()
        {
            self.waiters()
                .entry(guild_id)
                .or_default()
                .push(Waiter::Info(guid));
        }
    }

    /// The player watcher on `PLAYER_GUILD_TIMESTAMP` (`0x5e2890`) past its lookup: a stamp equal
    /// to the guild's last does nothing (`0x5e28fa`); any other is kept (`0x5e297e`, `0x5e298a`),
    /// the guild's record dropped (`0x5e2994`) and asked again with `0x5e29c0` (`0x5e29ac`).
    pub(crate) fn stamp_changed(&mut self, guild_id: u32, stamp: u32, commands: &NetCommands) {
        match self.stamps.iter_mut().find(|(g, _)| *g == guild_id) {
            Some((_, last)) if *last == stamp => return,
            Some((_, last)) => *last = stamp,
            None => self.stamps.push((guild_id, stamp)),
        }
        self.drop_record(guild_id);
        // The zero key asks nothing and keeps no callback (`0x560e3d`).
        if guild_id != 0 {
            self.resolve_identity(guild_id, Ask::Event(commands));
            self.waiters()
                .entry(guild_id)
                .or_default()
                .push(Waiter::Members);
        }
    }

    /// `SMSG_GUILD_QUERY_RESPONSE`: fill the identity cache. An empty name is a refusal: the
    /// reference frees the node and runs the callbacks with status 0 (`0x5552e1`, `0x561390`).
    /// Returns the callbacks a found record runs, in order.
    fn apply_query_response(&mut self, response: GuildQueryResponse) -> Vec<Waiter> {
        let ours = response.guild_id == self.guild_id;
        let found = !response.name.is_empty();
        let waiters = self
            .waiters()
            .remove(&response.guild_id)
            .unwrap_or_default();
        // A refusal redisplays only an identity it took away: every guild callback ignores
        // status 0 (`0x4d1486`, `0x4c9476`, `0x5dd866`, `0x5e29c6`, the crest's `0x5e0650` at
        // `0x5e0679`, the glue's `0x472420` at `0x472435`), and the corpse's `0x5d6f90` only
        // re-runs its read-only install (`0x5d6ec0(0)` at `0x5d6fba`).
        let moved = if found {
            self.identities.insert(
                response.guild_id,
                Identity {
                    name: response.name,
                    rank_names: response.rank_names,
                    emblem: GuildEmblem {
                        emblem_style: response.emblem_style,
                        emblem_color: response.emblem_color,
                        border_style: response.border_style,
                        border_color: response.border_color,
                        background_color: response.background_color,
                    },
                },
            );
            true
        } else {
            self.identities.refuse(response.guild_id) == Refused::Record
        };
        if moved {
            self.identity_generation = self.identity_generation.wrapping_add(1);
            self.dirty = true;
        }
        if ours && found {
            // Our rank names landed: repaint, never re-request, as the reference's cache callback
            // `0x4d1480` does for a found record only (`0x4d1486`, `0x4d148c`).
            self.note_roster_update(RosterUpdate::Applied);
        }
        if found {
            waiters
        } else {
            Vec::new()
        }
    }

    /// `SMSG_GUILD_ROSTER`: a complete snapshot, never a delta.
    fn apply_roster(&mut self, roster: GuildRoster) {
        self.motd = roster.motd;
        self.info_text = roster.info;
        // Deviation: clamped to ten, because the reference's unbounded loop (`0x4d0bb0`) overruns
        // its ten-slot array on a hostile `rankCount >= 12`.
        self.rank_rights = roster.rank_rights;
        self.rank_rights.truncate(GUILD_RANKS_MAX_COUNT);
        self.members = roster.members;
        self.note_roster_update(RosterUpdate::Applied);
    }

    /// `SMSG_GUILD_EVENT`'s effect on the mirror; its line is [`lines`]'.
    fn apply_event(&mut self, notice: &GuildEventNotice) {
        match notice.event {
            guild_event::MOTD => {
                // Stored so `GetGuildRosterMOTD` agrees with the event before the next roster.
                self.motd = notice.params.first().cloned().unwrap_or_default();
                self.dirty = true;
            }
            guild_event::UPDATE_RANK_NAME => {
                // Written into the cached record with no event of its own (`0x560e30`); params are
                // the rank id as text, then the name (vmangos `Guild/Guild.h:133`).
                let rank = notice.params.first().and_then(|p| p.parse::<usize>().ok());
                let name = notice.params.get(1).cloned().unwrap_or_default();
                if let (Some(rank), Some(identity)) = (rank, self.identities.get_mut(self.guild_id))
                {
                    if let Some(slot) = identity.rank_names.get_mut(rank) {
                        *slot = name;
                        self.dirty = true;
                    }
                }
            }
            _ => {}
        }
        // Only cases `0x00`-`0x0b` carry arg1: not sign-on or sign-off, nor anything past them.
        if notice.event <= guild_event::UPDATE_ROSTER {
            self.note_roster_update(RosterUpdate::Stale);
        }
    }

    /// `SMSG_GUILD_COMMAND_RESULT`'s effect on the mirror: stale only for a successful command
    /// `0x13`/`0x14` or result `0x14` on command `0x05` (`0x5e7792`); the server re-sends the
    /// roster for any real change.
    fn apply_command_result(&mut self, result: &GuildCommandResult) {
        use benilla_protocol::messages::{guild_command, guild_command_error};
        let stale = match result.result {
            guild_command_error::PLAYER_NO_MORE_IN_GUILD => {
                matches!(result.command, guild_command::UNK19 | guild_command::UNK20)
            }
            guild_command_error::UNK20 => result.command == 0x05,
            _ => false,
        };
        if stale {
            self.note_roster_update(RosterUpdate::Stale);
        }
    }

    /// The member at a 1-based display row (`[0xb72704]` at `index - 1`, `0x4d1633`).
    fn member_at(&self, local: &GuildLocal, index: u32) -> Option<&GuildRosterMember> {
        let guid = local.guid_at(index)?;
        self.members.iter().find(|m| m.guid == guid)
    }

    /// Whether a member is held (`[0xb73118]`), which `0x4d0f70` tests before its sort
    /// (`0x4d0f85`).
    fn has_members(&self) -> bool {
        !self.members.is_empty()
    }

    /// `GetNumGuildMembers()`: the whole roster with show-offline on, else the online tally
    /// (`0x4d1190`, against `0xb73118`/`0xb7311c`). A loop bound, not the addressable range: with
    /// show-offline off the offline rows sort last ([`sort`]).
    fn num_members(&self, show_offline: i32) -> usize {
        if show_offline != 0 {
            self.members.len()
        } else {
            self.members.iter().filter(|m| m.is_online()).count()
        }
    }

    /// Our rank's rights word, which every `Can*` predicate tests; `0` (none) before a roster.
    fn own_rights(&self) -> u32 {
        self.rank_rights
            .get(self.rank_index as usize)
            .copied()
            .unwrap_or(0)
    }
}

/// Days since logout, fully decomposed as `GetGuildRosterLastOnline 0x4d14a0` does
/// (`0x4d1508`-`0x4d1598`), each step truncated (`_ftol` `0x40a2b0`) and the remainder after
/// months narrowed to f32 (`0x4d1544`). The stock frame shows only the largest non-zero unit, and
/// all zeroes as under an hour (`FriendsFrame.lua:957-978`).
pub(crate) fn last_online(days: f32) -> LastOnline {
    if days.is_nan() || days <= 0.0 {
        // NaN, negative, and an online member's absent float: all zeroes.
        return LastOnline::default();
    }
    // f64 for the reference's x87 extended precision; it multiplies by f32 reciprocals (1/365 at
    // `0x80733c`, 1/30 at `0x807334`; 365, 30 and 24 at `0x807338`, `0x807330`, `0x80732c`).
    const PER_YEAR: f32 = 1.0 / 365.0;
    const PER_MONTH: f32 = 1.0 / 30.0;
    let days = f64::from(days);
    let years = (days * f64::from(PER_YEAR)) as u32;
    let rem = days - f64::from(years) * 365.0;
    let months = (rem * f64::from(PER_MONTH)) as u32;
    let rem = (rem - f64::from(months) * 30.0) as f32;
    let day = rem as u32;
    LastOnline {
        years,
        months,
        days: day,
        hours: ((rem - day as f32) * 24.0) as u32,
    }
}

/// `GetGuildInfo("player")`'s leg: our public guild fields joined against the identity cache,
/// asking on a miss. `None` for a guildless unit, a creature or an unanswered query, one nil path
/// in the reference (`0x4c93d7`, `0x4c943c`). The getter's own lookup asks with the callback
/// `0x4c9470` (`0x4c93d2`); this one asks when the guild first shows, and a refused one again at
/// the guild id's next change (the equipment resolver's lookup, `0x5e078f`).
pub(crate) fn unit_guild(
    fields: &ObjectFields,
    guild: &GuildState,
    commands: &NetCommands,
) -> Option<UnitGuild> {
    guild_of(fields, guild, Ask::New(commands))
}

/// [`unit_guild`] from the cache alone, asking nothing: the guild leg of a held unit's snapshot,
/// whose `GetGuildInfo` asks a miss itself, with a waiter of its own, inside its call
/// (`0x4c93d2`, [`benilla_ui::script::RecordAsk::Guild`]).
pub(crate) fn cached_unit_guild(fields: &ObjectFields, guild: &GuildState) -> Option<UnitGuild> {
    guild_of(fields, guild, Ask::Read)
}

fn guild_of(fields: &ObjectFields, guild: &GuildState, ask: Ask<'_>) -> Option<UnitGuild> {
    let guild_id = fields.player_guild_id();
    let rank_index = fields.player_guild_rank();
    let identity = guild.resolve_identity(guild_id, ask)?;
    Some(UnitGuild {
        name: identity.name.clone(),
        rank_name: identity.rank_name(rank_index).to_string(),
        rank_index,
    })
}

/// A unit's guild name alone: the overhead name stack's guild line (`"\n<%s>"`, `0x860f9c`), shown
/// under `UnitNamePlayerGuild`'s mask bit `0x10` (`0x609085`) and read through `0x5e09f0`.
/// [`unit_guild`] without its clones: [`crate::nameplates::drive_nameplates`] reads it per frame.
pub(crate) fn unit_guild_name<'a>(
    fields: &ObjectFields,
    guild: &'a GuildState,
    commands: &NetCommands,
) -> Option<&'a str> {
    guild
        .resolve_identity(fields.player_guild_id(), Ask::New(commands))
        .map(|identity| identity.name.as_str())
}

/// A unit's guild crest for the body composite, off its public `PLAYER_GUILDID`, its guild record
/// looked up as `ask` says. `None` keeps a Guild Tabard's own `Tabard_A_05Default` art, as the
/// reference never enters its install (`0x47a610`) for a guildless wearer (`0x560e30` returns NULL
/// at `0x560e3f`), a creature, a query in flight (re-dressed when
/// [`GuildState::identity_generation`] moves; the reference's callback is `0x5e0650`) or an
/// undesigned `-1` crest, which would paint blank.
pub(crate) fn unit_guild_emblem(
    fields: &ObjectFields,
    guild: &GuildState,
    ask: Ask<'_>,
) -> Option<GuildEmblem> {
    guild_emblem(fields.player_guild_id(), guild, ask)
}

/// The same crest for a corpse, off its own `CORPSE_FIELD_GUILD` (`0x5d6edf`) through the same
/// lookup (`0x5d6ec0`), with the same `None` cases.
pub(crate) fn corpse_guild_emblem(
    fields: &ObjectFields,
    guild: &GuildState,
    ask: Ask<'_>,
) -> Option<GuildEmblem> {
    guild_emblem(fields.corpse_guild(), guild, ask)
}

fn guild_emblem(guild_id: u32, guild: &GuildState, ask: Ask<'_>) -> Option<GuildEmblem> {
    let emblem = guild.resolve_identity(guild_id, ask)?.emblem;
    emblem.is_designed().then_some(emblem)
}

/// The guild packet handlers: they update the mirror and queue the message ids [`lines`] names,
/// whose catalog rows pick the surface and the sound; the guild events fire from the feed.
pub(crate) mod net {
    use super::*;
    use crate::field_watch::FieldNotice;
    use crate::ui_action::{UiError, UiErrorKeys};
    use crate::ui_social::SocialState;
    use benilla_protocol::messages::ObjectType;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

    /// Register the guild handlers, one per kind, plus the session-end listener.
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::GuildQueryResponse, on_query_response)
            .net_handler(K::GuildRoster, on_roster)
            .net_handler(K::GuildEvent, on_event)
            .net_handler(K::GuildCommandResult, on_command_result)
            .net_handler(K::GuildInvite, on_invite)
            .net_handler(K::GuildDecline, on_decline)
            .net_handler(K::GuildInfo, on_info)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// Reads the held players for the timestamp's refresh ([`query_response`]).
    fn on_query_response(
        In(ev): In<SessionEvent>,
        mut guild: ResMut<GuildState>,
        index: Res<crate::net::GuidIndex>,
        stores: Query<(&crate::net::ObjectStore, Option<&crate::net::Listed>)>,
        mut notices: ResMut<crate::field_watch::NoticeLane>,
    ) {
        if let SessionEvent::GuildQueryResponse(response) = ev {
            let guild_id = response.guild_id;
            let waiters = query_response(&mut guild, response);
            // The held players in object-list order, as the refresh walk meets them (`0x468380`).
            let mut players: Vec<(Option<crate::net::Listed>, u64, u32)> = index
                .0
                .iter()
                .filter_map(|(&g, &e)| {
                    let (store, listed) = stores.get(e).ok()?;
                    (store.0.created_as() == Some(ObjectType::Player))
                        .then(|| (listed.copied(), g, store.0.player_guild_id()))
                })
                .collect();
            players.sort_unstable();
            let players: Vec<(u64, u32)> = players.into_iter().map(|(_, g, id)| (g, id)).collect();
            notices.write_batch(waiter_notices(guild_id, &waiters, &players));
        }
    }

    /// The notices a found record's callbacks raise, in their order: a `GetGuildInfo` waiter's
    /// `PLAYER_GUILD_UPDATE` for its unit (`0x4c947c`-`0x4c9481`); the timestamp's refresh walk
    /// (`0x5e29c0` → `0x468380`), `RefreshGuildTag` for each held player, `(guid, guild)`, whose
    /// `PLAYER_GUILDID` is the guild (`0x5e2a14`-`0x5e2a1d` → `0x5e08e0`).
    fn waiter_notices(
        guild_id: u32,
        waiters: &[Waiter],
        players: &[(u64, u32)],
    ) -> Vec<FieldNotice> {
        let mut out = Vec::new();
        for waiter in waiters {
            match *waiter {
                Waiter::Info(guid) => out.push(FieldNotice::Signal {
                    guid,
                    id: crate::field_watch::id::PLAYER_GUILD_UPDATE,
                }),
                Waiter::Members => out.extend(
                    players
                        .iter()
                        .filter(|&&(_, g)| g == guild_id)
                        .flat_map(|&(guid, _)| crate::field_watch::guild_tag(guid)),
                ),
            }
        }
        out
    }

    fn on_roster(In(ev): In<SessionEvent>, mut guild: ResMut<GuildState>) {
        if let SessionEvent::GuildRoster(r) = ev {
            roster(&mut guild, r);
        }
    }

    /// Reads the friends list, the notify CVar and our guid for the sign-on condition ([`event`]).
    fn on_event(
        In(ev): In<SessionEvent>,
        mut guild: ResMut<GuildState>,
        errors: Res<UiErrorKeys>,
        social: Res<SocialState>,
        notify: Res<GuildMemberNotify>,
        self_guid: Res<crate::net::SelfGuid>,
    ) {
        if let SessionEvent::GuildEvent(notice) = ev {
            event(&mut guild, &errors, &social, &notify, self_guid.0, notice);
        }
    }

    fn on_command_result(
        In(ev): In<SessionEvent>,
        mut guild: ResMut<GuildState>,
        errors: Res<UiErrorKeys>,
    ) {
        if let SessionEvent::GuildCommandResult(result) = ev {
            command_result(&mut guild, &errors, result);
        }
    }

    fn on_invite(
        In(ev): In<SessionEvent>,
        mut guild: ResMut<GuildState>,
        mut local: ResMut<GuildLocal>,
        errors: Res<UiErrorKeys>,
    ) {
        if let SessionEvent::GuildInvite { inviter, guild: g } = ev {
            invite(&mut guild, &mut local, &errors, inviter, g);
        }
    }

    fn on_decline(In(ev): In<SessionEvent>, errors: Res<UiErrorKeys>) {
        if let SessionEvent::GuildDecline { name } = ev {
            decline(&errors, &name);
        }
    }

    fn on_info(In(ev): In<SessionEvent>, mut guild: ResMut<GuildState>) {
        if let SessionEvent::GuildInfo(i) = ev {
            info(&mut guild, i);
        }
    }

    /// Reset at session end: the next login may be another character. The identity store goes
    /// too, which the next world entry would clear anyway (`0x5557c0`).
    fn on_session_end(
        In(_): In<SessionEvent>,
        mut guild: ResMut<GuildState>,
        mut local: ResMut<GuildLocal>,
    ) {
        *guild = GuildState::default();
        *local = GuildLocal::default();
    }

    /// Queue the message ids [`lines`] named.
    fn push_lines(errors: &UiErrorKeys, lines: impl IntoIterator<Item = UiError>) {
        errors.extend(lines);
    }

    /// `SMSG_GUILD_QUERY_RESPONSE`: the callbacks a found record runs.
    pub(crate) fn query_response(
        guild: &mut GuildState,
        response: GuildQueryResponse,
    ) -> Vec<Waiter> {
        guild.apply_query_response(response)
    }

    /// `SMSG_GUILD_ROSTER`.
    pub(crate) fn roster(guild: &mut GuildState, roster: GuildRoster) {
        guild.apply_roster(roster);
    }

    /// `SMSG_GUILD_EVENT`. The sign-on/sign-off pair's guid serves its line's condition, four
    /// conjuncts in the reference's `0x0c`/`0x0d` arms, each failing to the silent exit `0x5e74c9`:
    ///
    /// 1. a local player exists (ours: we know our guid);
    /// 2. `guildMemberNotify` is on (`0x5e733f`, `0x5e73e7`);
    /// 3. the subject is not us: the reference compares names (`0x609210`), we compare guids, and
    ///    vmangos sends the sign-on to the signer too (`Guild.cpp:651-656`);
    /// 4. the subject is not a friend (`FriendList::FindFriendSlot 0x5ae810`, not the ignore
    ///    check), since `SMSG_FRIEND_STATUS` prints the same line ungated (`0x5acde6`, `0x5ace08`).
    pub(crate) fn event(
        guild: &mut GuildState,
        errors: &UiErrorKeys,
        social: &SocialState,
        notify: &GuildMemberNotify,
        self_guid: Option<u64>,
        notice: GuildEventNotice,
    ) {
        let announce = announce_signon(social, notify, self_guid, notice.guid);
        guild.apply_event(&notice);
        push_lines(errors, lines::event_line(&notice, announce));
    }

    /// The sign-on/sign-off line's four conjuncts ([`event`]) as one predicate.
    pub(super) fn announce_signon(
        social: &SocialState,
        notify: &GuildMemberNotify,
        self_guid: Option<u64>,
        subject: Option<u64>,
    ) -> bool {
        if !notify.0 {
            return false; // conjunct 2
        }
        match (self_guid, subject) {
            // conjuncts 3 and 4.
            (Some(me), Some(subject)) => subject != me && !social.is_friend(subject),
            // No local player (conjunct 1) or no guid on the wire: the silent exit.
            _ => false,
        }
    }

    /// `SMSG_GUILD_COMMAND_RESULT`.
    pub(crate) fn command_result(
        guild: &mut GuildState,
        errors: &UiErrorKeys,
        result: GuildCommandResult,
    ) {
        guild.apply_command_result(&result);
        push_lines(errors, lines::command_line(&result));
    }

    /// `SMSG_GUILD_INVITE`: the popup's show edge and the notice line printed beside it.
    pub(crate) fn invite(
        guild: &mut GuildState,
        local: &mut GuildLocal,
        errors: &UiErrorKeys,
        inviter: String,
        guild_name: String,
    ) {
        push_lines(errors, [lines::invite_line(&inviter, &guild_name)]);
        local.pending_invite = Some((inviter, guild_name));
        guild.dirty = true;
    }

    /// `SMSG_GUILD_DECLINE`: a line, no state.
    pub(crate) fn decline(errors: &UiErrorKeys, name: &str) {
        push_lines(errors, [lines::decline_line(name)]);
    }

    /// `SMSG_GUILD_INFO`, the `/ginfo` answer: parked for the feed, which builds its two lines
    /// ([`GuildState::pending_info`]).
    pub(crate) fn info(guild: &mut GuildState, info: GuildInfo) {
        guild.pending_info.push(info);
    }
}

/// The guild windows' session: the wire mirror, the VM feed, and the getter's missed lookups.
pub(crate) struct UiGuildPlugin;

/// `guildMemberNotify`'s change callback: conjunct 2 of the sign-on/sign-off line's condition.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut notify: ResMut<GuildMemberNotify>) {
    if ev.is("guildMemberNotify") {
        notify.0 = ev.flag();
    }
}

impl Plugin for UiGuildPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.add_observer(on_cvar);
        crate::query_cache::register::<GuildState>(app);
        app.init_resource::<GuildState>()
            .init_resource::<GuildLocal>()
            .init_resource::<GuildMemberNotify>()
            .add_systems(
                Update,
                // Its push is what `PLAYER_GUILD_UPDATE`'s handlers read.
                feed::feed_guild
                    .in_set(UiFeed)
                    .in_set(crate::ui_unit::field_events::FieldEventPushes)
                    // Never against the glue VM: the feed's edge memo survives the entry load, so
                    // an edge spent on a VM with no frames, such as the login MOTD, is lost.
                    .run_if(crate::ui_script::ingame_ui_up),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_social::SocialState;
    use benilla_protocol::messages::{guild_command, guild_command_error, guild_presence};

    fn identity(name: &str, ranks: &[&str]) -> Identity {
        let mut rank_names: [String; GUILD_RANKS_MAX_COUNT] = Default::default();
        for (slot, name) in rank_names.iter_mut().zip(ranks) {
            *slot = (*name).to_string();
        }
        Identity {
            name: name.to_string(),
            rank_names,
            emblem: GuildEmblem::default(),
        }
    }

    fn member(guid: u64, name: &str, presence: u8) -> GuildRosterMember {
        GuildRosterMember {
            guid,
            presence,
            name: name.to_string(),
            level: 60,
            ..Default::default()
        }
    }

    fn event(event: u8, params: &[&str], guid: Option<u64>) -> GuildEventNotice {
        GuildEventNotice {
            event,
            params: params.iter().map(|p| (*p).to_string()).collect(),
            guid,
        }
    }

    #[test]
    fn the_signon_condition_is_all_four_conjuncts() {
        let mut social = SocialState::default();
        crate::ui_social::net::friend_list(
            &mut social,
            vec![benilla_protocol::messages::FriendEntry {
                guid: 7,
                ..Default::default()
            }],
            &crate::ui_social::NoNames,
        );
        crate::ui_social::net::ignore_list(&mut social, vec![9], &crate::ui_social::NoNames);
        let on = GuildMemberNotify(true);
        let off = GuildMemberNotify(false);
        let me = Some(1);

        // 2: the CVar, off by default.
        assert!(
            !net::announce_signon(&social, &off, me, Some(5)),
            "guildMemberNotify off silences the whole family"
        );
        assert!(net::announce_signon(&social, &on, me, Some(5)));

        // 3: not us; vmangos sends the sign-on to the signer too (`Guild.cpp:651-656`).
        assert!(
            !net::announce_signon(&social, &on, me, Some(1)),
            "your own sign-on is not announced to you"
        );

        // 4: not a friend, whose `SMSG_FRIEND_STATUS` prints the same line.
        assert!(
            !net::announce_signon(&social, &on, me, Some(7)),
            "a guildmate who is also a friend is announced by the friend path, not twice"
        );

        // An ignored guildmate is announced: the ignore list plays no part.
        assert!(
            social.is_ignored(9),
            "the fixture's ignore really is an ignore"
        );
        assert!(
            net::announce_signon(&social, &on, me, Some(9)),
            "the reference announces an ignored guildmate — 0x5ae810 is not the ignore check"
        );

        // 1: a local player, and the pair's guid; either missing is the silent exit.
        assert!(!net::announce_signon(&social, &on, None, Some(5)));
        assert!(!net::announce_signon(&social, &on, me, None));
    }

    #[test]
    fn show_offline_changes_the_count_and_the_order_not_the_membership() {
        let mut guild = GuildState::default();
        guild.apply_roster(GuildRoster {
            members: vec![
                member(1, "Zed", guild_presence::ONLINE),
                member(2, "Gone", guild_presence::OFFLINE),
                member(3, "Away", guild_presence::ONLINE | guild_presence::AFK),
            ],
            ..Default::default()
        });

        let local = GuildLocal::default();
        let rows = feed::display_rows(&guild, &local, 0, None, None, Default::default());
        assert_eq!(rows.len(), 3, "every member, always");
        assert_eq!(guild.num_members(0), 2, "but the count is the online tally");
        assert!(
            rows[..2].iter().all(|r| r.info.online),
            "and the counted rows are the leading ones"
        );
        assert!(!rows[2].info.online);

        let rows = feed::display_rows(&guild, &local, 1, None, None, Default::default());
        assert_eq!(rows.len(), 3);
        assert_eq!(guild.num_members(1), 3, "now the count is everybody");
        assert_eq!(guild.num_members(2), 3, "any flag but 0 (`0x4d11ab`)");
    }

    #[test]
    fn last_online_decomposes_largest_unit_first() {
        // Under an hour: every unit zero.
        assert_eq!(last_online(0.02), LastOnline::default());
        assert_eq!(last_online(0.0), LastOnline::default());
        // Just over an hour.
        assert_eq!(
            last_online(1.0 / 24.0 + 0.001),
            LastOnline {
                hours: 1,
                ..Default::default()
            }
        );
        // Just under and just over a day.
        assert_eq!(last_online(0.99).days, 0);
        assert_eq!(last_online(0.99).hours, 23);
        assert_eq!(last_online(1.01).days, 1);
        // Just under and just over a month (30 days).
        assert_eq!(last_online(29.9).months, 0);
        assert_eq!(last_online(29.9).days, 29);
        assert_eq!(
            last_online(30.0),
            LastOnline {
                months: 1,
                ..Default::default()
            }
        );
        // Just under and just over a year (365 days).
        assert_eq!(last_online(364.9).years, 0);
        assert_eq!(last_online(364.9).months, 12, "12 months, not 1 year");
        assert_eq!(
            last_online(365.0),
            LastOnline {
                years: 1,
                ..Default::default()
            }
        );
        // The full decomposition: 400.5 days is 1 year, 1 month, 5 days, 12 hours.
        assert_eq!(
            last_online(400.5),
            LastOnline {
                years: 1,
                months: 1,
                days: 5,
                hours: 12,
            }
        );
    }

    #[test]
    fn an_empty_query_name_is_a_refusal_not_a_blank_guild() {
        let (commands, rx) = net_commands();
        let mut guild = GuildState::default();

        assert!(guild.resolve_identity(7, Ask::New(&commands)).is_none());
        assert!(guild.resolve_identity(7, Ask::New(&commands)).is_none());
        assert_eq!(rx.try_iter().count(), 1, "asked exactly once");

        let generation = guild.identity_generation();
        guild.dirty = false;
        guild.apply_query_response(GuildQueryResponse {
            guild_id: 7,
            name: String::new(),
            ..Default::default()
        });
        assert!(guild.identities.answered_unknown(7), "refused, not stored");
        assert_eq!(
            (guild.identity_generation(), guild.dirty),
            (generation, false),
            "nothing redisplays: every guild callback ignores status 0"
        );
        assert!(!guild.identities.is_pending(7), "and no longer in flight");
        assert!(guild.identities.get(7).is_none());
        assert!(
            guild.resolve_identity(7, Ask::New(&commands)).is_none(),
            "an empty name is not a guild"
        );
        assert_eq!(
            rx.try_iter().count(),
            0,
            "a reader's ask within the window of the refusal is not sent"
        );
    }

    #[test]
    fn a_guild_event_asks_again_and_a_roster_does_not() {
        let mut guild = GuildState::default();

        guild.apply_event(&event(guild_event::JOINED, &["Furor"], None));
        assert_eq!(guild.roster_event.take(), Some(RosterUpdate::Stale));
        assert!(RosterUpdate::Stale.arg1(), "the pane re-requests");
        assert!(
            !RosterUpdate::Applied.arg1(),
            "a repaint must never ask the server for a roster"
        );

        guild.apply_roster(GuildRoster::default());
        assert_eq!(
            guild.roster_event.take(),
            Some(RosterUpdate::Applied),
            "the roster we just applied is the one we have — asking for it again is the loop"
        );

        // Both in one frame: the re-request subsumes the repaint.
        guild.apply_roster(GuildRoster::default());
        guild.apply_event(&event(guild_event::PROMOTION, &["A", "B", "Officer"], None));
        assert_eq!(guild.roster_event.take(), Some(RosterUpdate::Stale));
    }

    #[test]
    fn signing_on_and_off_does_not_re_request() {
        let mut guild = GuildState::default();
        for ev in [guild_event::SIGNED_ON, guild_event::SIGNED_OFF] {
            guild.roster_event = None;
            guild.apply_event(&event(ev, &["Tigole"], Some(9)));
            assert_eq!(guild.roster_event, None, "event {ev:#04x}");
        }
        // The case just below them does.
        guild.apply_event(&event(guild_event::UPDATE_ROSTER, &[], None));
        assert_eq!(guild.roster_event, Some(RosterUpdate::Stale));
    }

    #[test]
    fn most_command_results_do_not_move_the_roster() {
        let mut guild = GuildState::default();
        guild.apply_command_result(&GuildCommandResult {
            command: guild_command::INVITE,
            name: "Kaplan".into(),
            result: guild_command_error::PERMISSIONS,
        });
        assert_eq!(guild.roster_event, None);

        guild.apply_command_result(&GuildCommandResult {
            command: guild_command::UNK19,
            name: String::new(),
            result: guild_command_error::PLAYER_NO_MORE_IN_GUILD,
        });
        assert_eq!(guild.roster_event, Some(RosterUpdate::Stale));
    }

    #[test]
    fn leaving_the_guild_drops_the_roster_but_not_the_identities() {
        let mut guild = GuildState::default();
        guild.identities.insert(7, identity("Legacy", &["GM"]));
        assert!(guild.mirror_self(7, 0), "joined");
        guild.apply_roster(GuildRoster {
            motd: "Raid at eight".into(),
            rank_rights: vec![0xffff, 0x3],
            members: vec![member(1, "Tigole", guild_presence::ONLINE)],
            ..Default::default()
        });
        assert!(guild.in_guild());

        assert!(guild.mirror_self(0, 0), "left");
        assert!(!guild.in_guild());
        assert!(guild.members.is_empty());
        assert!(guild.motd.is_empty());
        assert!(guild.identities.answered(7), "identities survive");

        assert!(!guild.mirror_self(0, 0), "no edge when nothing moved");
    }

    /// At login the MOTD event arrives before our descriptor names the guild.
    #[test]
    fn learning_our_own_guild_id_is_not_leaving_a_guild() {
        let mut guild = GuildState::default();
        guild.apply_event(&event(
            guild_event::MOTD,
            &["Raid Wednesday at eight."],
            None,
        ));
        assert!(!guild.in_guild(), "the descriptor has not said yet");

        assert!(
            guild.mirror_self(1, 3),
            "the descriptor finally names the guild"
        );
        assert_eq!(
            guild.motd, "Raid Wednesday at eight.",
            "the MOTD is about the guild we have just been told we are in"
        );
        assert_eq!(
            guild.roster_event,
            Some(RosterUpdate::Stale),
            "the pane still learns the snapshot moved — the MOTD packet's own stale signal, \
             which outranks the `Applied` this edge notes"
        );
    }

    #[test]
    fn own_rights_index_through_the_descriptor_rank() {
        let mut guild = GuildState::default();
        guild.mirror_self(7, 1);
        assert_eq!(guild.own_rights(), 0, "no roster yet");
        guild.apply_roster(GuildRoster {
            rank_rights: vec![0xffff, 0x00ff, 0x000f],
            ..Default::default()
        });
        assert_eq!(guild.own_rights(), 0x00ff);
        assert_eq!(guild.rank_rights.len(), 3);
        guild.mirror_self(7, 9); // a rank past the array
        assert_eq!(guild.own_rights(), 0);
    }

    #[test]
    fn an_over_long_rank_array_is_clamped() {
        let mut guild = GuildState::default();
        guild.apply_roster(GuildRoster {
            rank_rights: vec![1; 14],
            ..Default::default()
        });
        assert_eq!(guild.rank_rights.len(), GUILD_RANKS_MAX_COUNT);
    }

    #[test]
    fn a_rank_rename_patches_the_identity_cache() {
        let mut guild = GuildState::default();
        guild.mirror_self(7, 0);
        guild
            .identities
            .insert(7, identity("Legacy", &["GM", "Off"]));
        guild.apply_event(&event(
            guild_event::UPDATE_RANK_NAME,
            &["1", "Officer"],
            None,
        ));
        assert_eq!(guild.identities.get(7).unwrap().rank_name(1), "Officer");
        assert!(guild.dirty);
    }

    #[test]
    fn a_display_row_resolves_to_its_member() {
        let mut guild = GuildState::default();
        let mut local = GuildLocal::default();
        guild.apply_roster(GuildRoster {
            members: vec![
                member(11, "Alice", guild_presence::ONLINE),
                member(22, "Bob", guild_presence::ONLINE),
            ],
            ..Default::default()
        });
        local.display_order = vec![11, 22];
        assert_eq!(
            guild.member_at(&local, 2).map(|m| m.name.clone()),
            Some("Bob".to_string()),
            "a display row resolves to the member the wire wants by name"
        );
        local.display_order = vec![22, 11];
        assert_eq!(guild.member_at(&local, 2).map(|m| m.guid), Some(11));
        assert!(guild.member_at(&local, 0).is_none());
        assert!(guild.member_at(&local, 3).is_none());
    }

    #[test]
    fn an_invitation_arms_and_answering_clears_it() {
        let mut guild = GuildState::default();
        let mut local = GuildLocal::default();
        net::invite(
            &mut guild,
            &mut local,
            &crate::ui_action::UiErrorKeys::default(),
            "Tigole".into(),
            "Legacy of Steel".into(),
        );
        assert_eq!(
            local.invite(),
            Some(&("Tigole".into(), "Legacy of Steel".into()))
        );
    }

    #[test]
    fn unit_guild_asks_on_a_miss_and_never_names_a_blank_guild() {
        let (commands, rx) = net_commands();
        let mut guild = GuildState::default();
        guild
            .identities
            .insert(7, identity("Legacy", &["GM", "Off"]));

        // A creature or a guildless player: no `PLAYER_GUILDID`.
        let guildless = ObjectFields::from_pairs(&[]);
        assert!(unit_guild(&guildless, &guild, &commands).is_none());
        assert_eq!(rx.try_iter().count(), 0, "guild id 0 asks nothing");

        // Fields 191/192 = PLAYER_GUILDID/PLAYER_GUILDRANK.
        let unknown = ObjectFields::from_pairs(&[(191, 9), (192, 1)]);
        assert!(
            unit_guild(&unknown, &guild, &commands).is_none(),
            "a query in flight is not a blank-named guild"
        );
        assert_eq!(rx.try_iter().count(), 1, "and the miss asked for it");

        let member = ObjectFields::from_pairs(&[(191, 7), (192, 1)]);
        let resolved = unit_guild(&member, &guild, &commands).expect("cached");
        assert_eq!(resolved.name, "Legacy");
        assert_eq!(resolved.rank_name, "Off");
        assert_eq!(resolved.rank_index, 1);

        // A rank past the ten slots names nothing.
        let odd_rank = ObjectFields::from_pairs(&[(191, 7), (192, 99)]);
        assert_eq!(
            unit_guild(&odd_rank, &guild, &commands).unwrap().rank_name,
            ""
        );
    }

    #[test]
    fn unit_guild_name_is_unit_guilds_name_on_every_leg() {
        let (commands, rx) = net_commands();
        let mut guild = GuildState::default();
        guild
            .identities
            .insert(7, identity("Legacy", &["GM", "Off"]));
        // An empty answer to our ask is a refusal: no such guild.
        guild.resolve_identity(8, Ask::New(&commands));
        let _ = rx.try_iter().count();
        guild.identities.refuse(8);

        let guildless = ObjectFields::from_pairs(&[]);
        assert_eq!(unit_guild_name(&guildless, &guild, &commands), None);
        assert_eq!(rx.try_iter().count(), 0, "guild id 0 asks nothing");

        let unknown = ObjectFields::from_pairs(&[(191, 9)]);
        assert_eq!(
            unit_guild_name(&unknown, &guild, &commands),
            None,
            "a query in flight draws no line"
        );
        assert_eq!(rx.try_iter().count(), 1, "and the miss asked for it");

        let blank = ObjectFields::from_pairs(&[(191, 8)]);
        assert_eq!(unit_guild_name(&blank, &guild, &commands), None);
        assert_eq!(
            rx.try_iter().count(),
            0,
            "a reader's ask within the window of the refusal asks nothing"
        );

        let member = ObjectFields::from_pairs(&[(191, 7), (192, 1)]);
        assert_eq!(unit_guild_name(&member, &guild, &commands), Some("Legacy"));
        let via_line = unit_guild_name(&member, &guild, &commands).map(str::to_owned);
        let via_api = unit_guild(&member, &guild, &commands).map(|g| g.name);
        assert_eq!(via_line, via_api, "the two readers of one cache disagree");
    }

    /// A command channel whose receiver stays alive, so a send neither blocks nor is dropped.
    /// `PLAYER_GUILD_UPDATE` is the descriptor watchers' (`crate::field_watch`, `0x5e2770`,
    /// `0x5e27b0`): the feed that mirrors our guild id and rank fires it on neither edge.
    #[test]
    fn the_guild_feed_fires_no_guild_update() {
        use crate::net::ObjectStore;
        use benilla_ui::script::UiScript;
        use bevy::ecs::system::RunSystemOnce;
        const PLAYER_GUILDID: u16 = 191;
        const PLAYER_GUILDRANK: u16 = 192;
        let mut app = App::new();
        app.init_resource::<GuildState>()
            .init_resource::<GuildLocal>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .insert_resource(net_commands().0);
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                SEEN = 0
                local f = CreateFrame("Frame")
                f:RegisterEvent("PLAYER_GUILD_UPDATE")
                f:SetScript("OnEvent", function() SEEN = SEEN + 1 end)
            "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        let me = app
            .world_mut()
            .spawn((
                crate::net::SelfPlayer,
                ObjectStore(benilla_protocol::ObjectFields::default()),
            ))
            .id();
        for (id, rank) in [(7, 0), (7, 3), (9, 3), (0, 0)] {
            app.world_mut()
                .entity_mut(me)
                .get_mut::<ObjectStore>()
                .unwrap()
                .0
                .merge(benilla_protocol::ObjectFields::from_pairs(&[
                    (PLAYER_GUILDID, id),
                    (PLAYER_GUILDRANK, rank),
                ]));
            app.world_mut().run_system_once(feed::feed_guild).unwrap();
            assert_eq!(app.world().resource::<GuildState>().guild_id, id);
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.resolve();
            assert_eq!(s.eval::<i64>("return SEEN").unwrap(), 0, "{id}/{rank}");
        }
    }

    /// The roster rebuild stands for the stock frame's `GetGuildRosterInfo`, whose lookup passes
    /// the callback `0x4d1480` (`0x4d1291`): a refused guild is asked by no frame that rebuilds
    /// nothing, only by the next change.
    #[test]
    fn a_refused_own_guild_is_asked_by_the_next_roster_change_never_by_a_frame() {
        use crate::net::ObjectStore;
        use benilla_ui::script::UiScript;
        const PLAYER_GUILDID: u16 = 191;
        let (commands, rx) = net_commands();
        let mut app = App::new();
        app.init_resource::<GuildState>()
            .init_resource::<GuildLocal>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .insert_resource(commands)
            .add_systems(Update, feed::feed_guild);
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.world_mut().spawn((
            crate::net::SelfPlayer,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                PLAYER_GUILDID,
                7,
            )])),
        ));
        let feed = |app: &mut App| app.update();
        feed(&mut app);
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
        app.world_mut()
            .resource_mut::<GuildState>()
            .identities
            .refuse(7);
        for _ in 0..600 {
            feed(&mut app);
        }
        assert!(
            crate::query_cache::queries(&rx).is_empty(),
            "no frame asks a refusal"
        );
        app.world_mut()
            .resource_mut::<GuildState>()
            .note_roster_update(RosterUpdate::Applied);
        feed(&mut app);
        assert_eq!(
            crate::query_cache::queries(&rx).len(),
            1,
            "the next change asks again"
        );
    }

    /// `GetGuildInfo` on a guild the cache lacks asks it with the unit's guid and the callback
    /// `0x4c9470`, dedupe off (`0x4c93b6`-`0x4c93d2`), so each missed call keeps a waiter of its
    /// own; the landing runs each, `PLAYER_GUILD_UPDATE` (0x159) for the guid through `0x515e50`
    /// (`0x4c947c`-`0x4c9481`). A refusal runs them with status 0, which signals nothing
    /// (`0x4c9476`).
    #[test]
    fn a_missed_get_guild_info_waits_for_the_record_and_signals_its_unit() {
        use crate::field_watch::FieldNotice;
        use benilla_protocol::SessionEvent;
        use benilla_ui::script::{UiScript, UnitState};
        const MATE: u64 = 0x2001;
        let (commands, rx) = net_commands();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<GuildState>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::field_watch::NoticeLane>()
            .insert_resource(commands)
            .add_systems(Update, feed::ask_kept_guilds);
        net::register(&mut app);
        let target = |guild_id: u32| UnitState {
            exists: true,
            has_object: true,
            guid: MATE,
            is_player: true,
            guild_id,
            ..Default::default()
        };
        let mut script = UiScript::new().unwrap();
        script.keep_calls();
        script.set_unit("target", Some(target(5)));
        app.insert_non_send_resource(script);
        let call = |app: &mut App| {
            let name = app
                .world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<Option<String>>("return (GetGuildInfo('target'))")
                .unwrap();
            assert_eq!(name, None, "the first answer of an uncached guild");
        };
        let drain = |app: &mut App| -> Vec<FieldNotice> {
            app.world_mut()
                .resource_mut::<crate::field_watch::NoticeLane>()
                .drain()
                .collect()
        };
        let answer = |guild_id: u32, name: &str| {
            vec![SessionEvent::GuildQueryResponse(GuildQueryResponse {
                guild_id,
                name: name.to_string(),
                ..Default::default()
            })]
        };
        call(&mut app);
        call(&mut app);
        app.update();
        assert!(matches!(
            crate::query_cache::queries(&rx).as_slice(),
            [ClientCommand::GuildQuery { guild_id: 5 }]
        ));
        crate::net::handlers::dispatch(app.world_mut(), answer(5, "Probe Guild"));
        let signal = FieldNotice::Signal {
            guid: MATE,
            id: crate::field_watch::id::PLAYER_GUILD_UPDATE,
        };
        assert_eq!(drain(&mut app), [signal; 2], "one per call that missed");
        crate::net::handlers::dispatch(app.world_mut(), answer(5, "Probe Guild"));
        assert!(drain(&mut app).is_empty(), "the waiters are spent");

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_unit("target", Some(target(6)));
        call(&mut app);
        app.update();
        crate::net::handlers::dispatch(app.world_mut(), answer(6, ""));
        assert!(drain(&mut app).is_empty(), "a refusal signals nothing");
    }

    fn net_commands() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }
}
