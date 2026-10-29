//! The guild session: the identity cache, the roster, the ranks and the outbound verbs.
//! [`GuildState`] mirrors the guild packets, the guild getters read it through the call host
//! ([`live`]) as of the place they run, and each packet's events fire from [`lane`] with the
//! state that packet left. The roster carries no guild or rank names: those come only from
//! `SMSG_GUILD_QUERY_RESPONSE`, cached by guild id and filled lazily, as in the reference.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use benilla_formats::GuildEmblem;
use benilla_protocol::messages::{
    guild_event, GuildCommandResult, GuildEventNotice, GuildInfo, GuildQueryResponse, GuildRoster,
    GUILD_RANKS_MAX_COUNT,
};
use benilla_protocol::ObjectFields;
use benilla_ui::script::{LastOnline, UiScript, UnitGuild};
use bevy::prelude::*;
use lane::{Fire, GuildLane};

use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::{Ask, QueryCache, Refused};

#[cfg(test)]
mod call_tests;
mod feed;
mod lane;
mod lines;
mod live;
mod sort;
mod steps;
mod view;

pub(crate) use live::GuildLive;
pub(crate) use sort::RosterRow;
use sort::{SortField, SortStack};
pub(crate) use steps::GuildSteps;
use view::GuildView;

/// The profanity masker (`0x4a1a60`) as the guild's handlers and getters hold it: it self-gates on
/// `profanityFilter` (`0x4a1a66`), and with no filter in the world masks nothing.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct Masker<'w> {
    filter: Option<Res<'w, crate::text_filter::TextFilter>>,
    switches: Option<Res<'w, crate::text_filter::TextFilterSwitches>>,
}

impl Masker<'_> {
    fn on(&self) -> bool {
        self.switches.as_deref().is_some_and(|s| s.profanity)
    }

    /// `dl = 0`: mask `text`, every call advancing the mask index past what it masks.
    fn plain(&self, text: &mut String) {
        if let Some(filter) = self.filter.as_deref() {
            filter.mask(self.on(), text);
        }
    }

    /// `dl = 1`: mask `text` through the masker's cache, a text masked before answered without
    /// moving the index.
    fn cached(&self, text: &mut String) {
        if let Some(filter) = self.filter.as_deref() {
            filter.mask_cached(self.on(), text);
        }
    }
}

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
    /// `GetGuildRosterInfo`'s and `GuildControlGetRankName`'s `0x4d1480` (`0x4d1291`, `0x4d1f29`):
    /// a bare `GUILD_ROSTER_UPDATE` (`0x4d148c`), one for each call that missed.
    Roster,
    /// The `PLAYER_GUILD_TIMESTAMP` watcher's `0x5e29c0` (`0x5e299d`): `RefreshGuildTag` on every
    /// held player of the guild (`0x5e29e0`).
    Members,
}

/// The guild session mirror: filled by the packet handlers, read by the getters through the call
/// host, reset on disconnect, and its roster and ranks at each world entry ([`seed_vm_guild`]).
#[derive(Resource)]
pub(crate) struct GuildState {
    /// Our `PLAYER_GUILDID` (field 191), `0` when guildless; mirrored from our descriptor as the
    /// packet that writes it ends ([`Self::mirror_self`]).
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
    /// What the getters read as the handlers left it: the roster message's state (the message of
    /// the day apart from it, which `GE_MOTD` sets alone: at login vmangos sends that event before
    /// any roster, `CharacterHandler.cpp:558`) and the guild records by id.
    now: GuildView,
    /// The events the handlers raised, each with the version it left ([`lane`]).
    lane: GuildLane,
    /// Real-time seconds (an `f64`'s bits) before which `GuildRoster()` is swallowed (`0xb73130`);
    /// `0.0` = allowed. The stale update's signal clears it (`0x4d1170`) and a world entry zeroes
    /// it (`0x4d0a41`).
    roster_allowed_at: AtomicU64,
}

impl Default for GuildState {
    fn default() -> Self {
        let now = GuildView::default();
        Self {
            guild_id: 0,
            rank_index: 0,
            identities: QueryCache::default(),
            identity_generation: 0,
            waiters: Mutex::new(HashMap::new()),
            stamps: Vec::new(),
            lane: GuildLane::new(now.clone()),
            now,
            roster_allowed_at: AtomicU64::new(0),
        }
    }
}

/// What the guild verbs write inside their calls ([`GuildSteps`]), apart from [`GuildState`] so
/// the unit and item feeds that read the guild cache beside the VM can share the call host: the
/// sort chain and the display order built from it. Reset at each world entry
/// ([`seed_vm_guild`]). The show-offline flag, the selection and the staged rank are the VM's
/// ([`benilla_ui::script::GuildCells`]).
#[derive(Resource, Default)]
pub(crate) struct GuildLocal {
    sort: SortStack,
    /// The display order last built, which a loop over the rows reads once ([`Self::ordered`]).
    order: live::OrderCache,
    /// The member notes as masked at their first read, for the member list they belong to.
    notes: live::NoteStore,
}

impl GuildLocal {
    /// `SortGuildRoster(field)`: the chain's step (`0x4d0fb0`); the caller re-sorts and signals.
    fn sort_by(&mut self, field: &str) {
        self.sort.select(SortField::parse(field));
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
    }
    if let Some(mut guild) = world.get_resource_mut::<GuildState>() {
        guild.allow_roster_request();
        guild.clear_members();
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
        Arc::make_mut(&mut self.now.identities).clear();
        self.settle();
    }

    /// The deadline `GuildRoster()` waits for (`0xb73130`), in real-time seconds.
    fn roster_allowed_at(&self) -> f64 {
        f64::from_bits(self.roster_allowed_at.load(Ordering::Relaxed))
    }

    /// Hold `GuildRoster()` until `at` (`0x4d10d0`).
    fn defer_roster_request(&self, at: f64) {
        self.roster_allowed_at
            .store(at.to_bits(), Ordering::Relaxed);
    }

    /// Allow `GuildRoster()` now: the stale update's signal clears the deadline (`0x4d1170`).
    fn allow_roster_request(&self) {
        self.defer_roster_request(0.0);
    }

    /// The version of the state the getters read: the live state, or while the lane drains the
    /// version the last item fired left ([`lane`]).
    pub(crate) fn seen(&self) -> &GuildView {
        self.lane.drained().unwrap_or(&self.now)
    }

    /// Queue what a handler signals with the version it just wrote, stamped here.
    fn raise(&mut self, fire: Fire) {
        self.lane.push(self.now.clone(), Some(fire));
    }

    /// Queue the version a write left that signals nothing, so a listener fired before it still
    /// reads the state ahead of it; nothing if the lane has seen this version.
    fn settle(&mut self) {
        if !self.lane.is_tail(&self.now) {
            self.lane.push(self.now.clone(), None);
        }
    }

    /// `InitGuildRoster`'s zeroed member, online and rank counts (`0x4d0a13`-`0x4d0a1f`); the
    /// message of the day and the information text it leaves.
    fn clear_members(&mut self) {
        let roster = Arc::make_mut(&mut self.now.roster);
        roster.rank_rights.clear();
        roster.set_members(Vec::new());
        self.settle();
    }

    /// The records by id as the cache holds them, after a write.
    fn publish_identity(&mut self, guild_id: u32) {
        let records = Arc::make_mut(&mut self.now.identities);
        match self.identities.get(guild_id) {
            Some(identity) => {
                records.insert(guild_id, Arc::new(identity.clone()));
            }
            None => {
                records.remove(&guild_id);
            }
        }
    }

    /// Our own `PLAYER_GUILDID` is set, known before any guild packet.
    #[cfg(test)]
    pub(crate) fn in_guild(&self) -> bool {
        self.guild_id != 0
    }

    /// Whether the mirror holds `guild_id` and `rank_index` already.
    pub(crate) fn mirrors(&self, guild_id: u32, rank_index: u32) -> bool {
        (self.guild_id, self.rank_index) == (guild_id, rank_index)
    }

    /// Mirror our descriptor's guild fields; `true` when either moved. Asks for nothing: the
    /// identity cache is lazy and the roster is the FrameXML's to request. Signals nothing and
    /// writes no roster state: the reference's roster update comes with the server's own guild
    /// event (`0x5e7180`).
    pub(crate) fn mirror_self(&mut self, guild_id: u32, rank_index: u32) -> bool {
        if self.mirrors(guild_id, rank_index) {
            return false;
        }
        // The roster stays across a move: only `InitGuildRoster` (`0x4d0a13`) and the roster
        // message (`0x4d0ae2`) write its counts, and the message its texts (`0x4d0b6d`).
        self.guild_id = guild_id;
        self.rank_index = rank_index;
        true
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

    /// Forgets the in-flight asks, as a dropped connection does, for a test elsewhere.
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
            self.publish_identity(guild_id);
        }
        self.settle();
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

    /// A roster getter missed `guild_id`: the lookup with its callback `0x4d1480` (`0x4d1291`,
    /// `0x4d1f29`), asking as [`Self::wait_for_info`] does and leaving a waiter, with no dedupe.
    pub(crate) fn wait_for_roster(&self, guild_id: u32, commands: &NetCommands) {
        if guild_id != 0
            && self
                .resolve_identity(guild_id, Ask::Event(commands))
                .is_none()
        {
            self.waiters()
                .entry(guild_id)
                .or_default()
                .push(Waiter::Roster);
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
        let guild_id = response.guild_id;
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
            self.publish_identity(guild_id);
        }
        // The roster getters' callbacks (`0x4d1480`) repaint, never re-request, for a found record
        // only (`0x4d1486`, `0x4d148c`): [`Waiter::Roster`], one for each call that missed.
        self.settle();
        if found {
            waiters
        } else {
            Vec::new()
        }
    }

    /// `SMSG_GUILD_ROSTER`: a complete snapshot, never a delta, which ends in a bare
    /// `GUILD_ROSTER_UPDATE` (`0x4d0ad0` → `0x4d0d3c`).
    fn apply_roster(&mut self, roster: GuildRoster) {
        let state = Arc::make_mut(&mut self.now.roster);
        state.motd = roster.motd;
        state.info_text = roster.info;
        // Deviation: clamped to ten, because the reference's unbounded loop (`0x4d0bb0`) overruns
        // its ten-slot array from a hostile `rankCount` of 11: index 10 is `0xb726f8`, a float
        // its initializer writes (`0x4d279c`).
        state.rank_rights = roster.rank_rights;
        state.rank_rights.truncate(GUILD_RANKS_MAX_COUNT);
        state.set_members(roster.members);
        self.raise(Fire::Roster(RosterUpdate::Applied));
    }

    /// `SMSG_GUILD_EVENT`'s effect on the mirror and its signals (`0x5e7180`): `GUILD_MOTD` for the
    /// message of the day (`0x5e728d`), which is not stored (`0xb72f00` is the roster message's
    /// alone, `0x4d0b6d`), then the roster update with arg1 (`0x5e74c4`); its line is [`lines`]'.
    fn apply_event(&mut self, notice: &GuildEventNotice) {
        // The jump table sends the sign-on and sign-off pair out without the signal, and the rank
        // rename past it when there is no player (`0x5e72b9`) or no cached record (`0x5e72e4`);
        // every other type, the unnamed ones above 13 included (`0x5e745a`), ends in `0x5e74c4`.
        let signals = match notice.event {
            guild_event::SIGNED_ON | guild_event::SIGNED_OFF => false,
            guild_event::UPDATE_RANK_NAME => self.identities.get(self.guild_id).is_some(),
            _ => true,
        };
        match notice.event {
            guild_event::MOTD => {
                let motd = notice.params.first().cloned().unwrap_or_default();
                self.raise(Fire::Motd(motd));
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
                        self.publish_identity(self.guild_id);
                    }
                }
            }
            _ => {}
        }
        if signals {
            self.raise(Fire::Roster(RosterUpdate::Stale));
        } else {
            self.settle();
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
            self.raise(Fire::Roster(RosterUpdate::Stale));
        }
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
            // The callbacks run in the order they were left (`0x556f91`): a roster getter's
            // repaint at its place, the notices of the others behind the lane's stamps.
            for waiter in &waiters {
                match waiter {
                    Waiter::Roster => guild.raise(Fire::Roster(RosterUpdate::Applied)),
                    other => notices.write_batch(waiter_notices(
                        guild_id,
                        std::slice::from_ref(other),
                        &players,
                    )),
                }
            }
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
                Waiter::Roster => {}
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

    fn on_roster(In(ev): In<SessionEvent>, mut guild: ResMut<GuildState>, masker: Masker) {
        if let SessionEvent::GuildRoster(r) = ev {
            roster(&mut guild, &masker, r);
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
        masker: Masker,
    ) {
        if let SessionEvent::GuildEvent(notice) = ev {
            event(
                &mut guild,
                &errors,
                &social,
                &notify,
                self_guid.0,
                &masker,
                notice,
            );
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
        errors: Res<UiErrorKeys>,
    ) {
        if let SessionEvent::GuildInvite { inviter, guild: g } = ev {
            invite(&mut guild, &errors, inviter, g);
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
    /// too, which the next world entry would clear anyway (`0x5557c0`). The verbs' own state
    /// resets at that entry ([`seed_vm_guild`]).
    fn on_session_end(In(_): In<SessionEvent>, mut guild: ResMut<GuildState>) {
        *guild = GuildState::default();
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

    /// `SMSG_GUILD_ROSTER`: the message of the day and the information text are masked as the
    /// message lands, in that order (`0x4d0b71`, `0x4d0b8e`, `dl = 1`).
    pub(crate) fn roster(guild: &mut GuildState, masker: &Masker, mut roster: GuildRoster) {
        masker.cached(&mut roster.motd);
        masker.cached(&mut roster.info);
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
        masker: &Masker,
        mut notice: GuildEventNotice,
    ) {
        // The message of the day is masked (`0x5e7277`, `dl = 0`) before it is signalled.
        if notice.event == guild_event::MOTD {
            if let Some(motd) = notice.params.first_mut() {
                masker.plain(motd);
            }
        }
        // Each arm's line (`0x5e745f`-`0x5e74b7`), then the roster update the arms but the
        // sign-on pair's end in (`0x5e74c4`); the message of the day's signal has no line.
        let announce = announce_signon(social, notify, self_guid, notice.guid);
        push_lines(errors, lines::event_line(&notice, announce));
        guild.apply_event(&notice);
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

    /// `SMSG_GUILD_INVITE` (`0x5e6f20`): the popup's show (`GUILD_INVITE_REQUEST`, `0x5e6f53` →
    /// `0x48f45c`), then the notice line (`0x5e6f65`). There is no hide edge: nothing in the 1.12
    /// client raises `GUILD_INVITE_CANCEL` (its wrapper `0x48f470` has no callers), and the accept
    /// and decline packets name no invitation, so none is held.
    pub(crate) fn invite(
        guild: &mut GuildState,
        errors: &UiErrorKeys,
        inviter: String,
        guild_name: String,
    ) {
        let line = lines::invite_line(&inviter, &guild_name);
        guild.raise(Fire::Invite {
            inviter,
            guild: guild_name,
        });
        push_lines(errors, [line]);
    }

    /// `SMSG_GUILD_DECLINE`: a line, no state.
    pub(crate) fn decline(errors: &UiErrorKeys, name: &str) {
        push_lines(errors, [lines::decline_line(name)]);
    }

    /// `SMSG_GUILD_INFO`, the `/ginfo` answer: its two chat lines print at the packet's place on
    /// the lane, where the VM's templates are (`0x5e6fb0`).
    pub(crate) fn info(guild: &mut GuildState, info: GuildInfo) {
        guild.raise(Fire::Info(info));
    }
}

/// The guild windows' session: the wire mirror, its lane, and the getters' lookups.
pub(crate) struct UiGuildPlugin;

/// `guildMemberNotify`'s change callback: conjunct 2 of the sign-on/sign-off line's condition.
pub(crate) fn on_cvar(
    ev: &crate::cvars::CvarChanged,
    mut notify: bevy::prelude::Mut<'_, GuildMemberNotify>,
) {
    if ev.is("guildMemberNotify") {
        notify.0 = ev.flag();
    }
}

impl Plugin for UiGuildPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<GuildState>()
            .init_resource::<GuildLocal>()
            .init_resource::<GuildMemberNotify>();
        lane::add_guild_lane(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_social::SocialState;
    use benilla_protocol::messages::{
        guild_command, guild_command_error, guild_presence, GuildRosterMember,
    };

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
        let roster = &guild.now.roster;
        let rows = local.ordered(roster, false, None, Default::default(), &|_| {});
        assert_eq!(rows.len(), 3, "every member, always");
        assert_eq!(roster.online, 2, "but the count is the online tally");
        assert!(
            rows[..2].iter().all(|r| r.info.online),
            "and the counted rows are the leading ones"
        );
        assert!(!rows[2].info.online);

        let rows = local.ordered(roster, true, None, Default::default(), &|_| {});
        assert_eq!(rows.len(), 3);
        assert_eq!(roster.members.len(), 3, "now the count is everybody");
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
        guild.lane.take_words();
        guild.apply_query_response(GuildQueryResponse {
            guild_id: 7,
            name: String::new(),
            ..Default::default()
        });
        assert!(guild.identities.answered_unknown(7), "refused, not stored");
        assert_eq!(
            (guild.identity_generation(), guild.lane.take_words()),
            (generation, Vec::<String>::new()),
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
        assert_eq!(guild.lane.take_words(), ["roster:stale"]);
        assert!(RosterUpdate::Stale.arg1(), "the pane re-requests");
        assert!(
            !RosterUpdate::Applied.arg1(),
            "a repaint must never ask the server for a roster"
        );

        guild.apply_roster(GuildRoster::default());
        assert_eq!(
            guild.lane.take_words(),
            ["roster"],
            "the roster we just applied is the one we have — asking for it again is the loop"
        );

        // Both in one frame: each packet signals at its own place (`0x4d0ad0`, `0x5e7180`).
        guild.apply_roster(GuildRoster::default());
        guild.apply_event(&event(guild_event::PROMOTION, &["A", "B", "Officer"], None));
        assert_eq!(guild.lane.take_words(), ["roster", "roster:stale"]);
    }

    /// The MOTD event signals `GUILD_MOTD` and then the stale roster update (`0x5e728d`,
    /// `0x5e74c4`); a roster that carries a new message signals only the bare update.
    #[test]
    fn the_motd_event_signals_the_text_and_then_the_stale_update() {
        let mut guild = GuildState::default();
        guild.apply_event(&event(guild_event::MOTD, &["Raid at eight"], None));
        assert_eq!(
            guild.lane.take_words(),
            ["motd:Raid at eight", "roster:stale"]
        );
        assert!(
            guild.now.roster.motd.is_empty(),
            "the event stores nothing: `0xb72f00` is the roster message's alone (`0x4d0b6d`)"
        );
        guild.apply_roster(GuildRoster {
            motd: "Raid at nine".into(),
            ..Default::default()
        });
        assert_eq!(
            guild.lane.take_words(),
            ["roster"],
            "a roster signals no GUILD_MOTD"
        );
    }

    #[test]
    fn signing_on_and_off_does_not_re_request() {
        let mut guild = GuildState::default();
        for ev in [guild_event::SIGNED_ON, guild_event::SIGNED_OFF] {
            guild.apply_event(&event(ev, &["Tigole"], Some(9)));
            assert!(guild.lane.take_words().is_empty(), "event {ev:#04x}");
        }
        // The case just below them does.
        guild.apply_event(&event(guild_event::UPDATE_ROSTER, &[], None));
        assert_eq!(guild.lane.take_words(), ["roster:stale"]);
    }

    #[test]
    fn most_command_results_do_not_move_the_roster() {
        let mut guild = GuildState::default();
        guild.apply_command_result(&GuildCommandResult {
            command: guild_command::INVITE,
            name: "Kaplan".into(),
            result: guild_command_error::PERMISSIONS,
        });
        assert!(guild.lane.take_words().is_empty());

        guild.apply_command_result(&GuildCommandResult {
            command: guild_command::UNK19,
            name: String::new(),
            result: guild_command_error::PLAYER_NO_MORE_IN_GUILD,
        });
        assert_eq!(guild.lane.take_words(), ["roster:stale"]);
    }

    /// Which event types end in the roster update with arg1: the jump table at `0x5e74dc` sends
    /// every type but the sign-on pair there, the unnamed 9 and those above 13 through the
    /// internal-error line (`0x5e745a`), and the rank rename past a missing player or record.
    /// A roster getter's lookup leaves a callback for each call that missed, with no dedupe
    /// (`0x4d1291`, `0x4d1f29`, `push 0`), and the query goes out once; a found record runs them
    /// all, a refusal none (`0x4d1486`).
    #[test]
    fn each_missed_roster_call_leaves_a_waiter_and_a_refusal_runs_none() {
        let (commands, rx) = net_commands();
        let mut guild = GuildState::default();
        guild.wait_for_roster(7, &commands);
        guild.wait_for_roster(7, &commands);
        assert_eq!(rx.try_iter().count(), 1, "asked once while pending");
        let found = |guild_id, name: &str| GuildQueryResponse {
            guild_id,
            name: name.into(),
            ..Default::default()
        };
        assert_eq!(
            guild.apply_query_response(found(7, "Legacy")),
            [Waiter::Roster, Waiter::Roster],
            "one callback for each call"
        );
        guild.wait_for_roster(7, &commands);
        assert!(guild.apply_query_response(found(7, "Legacy")).is_empty());
        guild.wait_for_roster(8, &commands);
        assert!(guild.apply_query_response(found(8, "")).is_empty());
        assert!(
            guild.lane.take_words().is_empty(),
            "the landing signals in its handler"
        );
    }

    #[test]
    fn every_event_type_signals_but_the_sign_on_pair_and_a_rename_without_a_record() {
        for ev in (0..=u8::MAX).filter(|e| *e != guild_event::UPDATE_RANK_NAME) {
            let mut guild = GuildState::default();
            guild.apply_event(&event(ev, &["a", "b", "c"], Some(9)));
            let words = guild.lane.take_words();
            let silent = matches!(ev, guild_event::SIGNED_ON | guild_event::SIGNED_OFF);
            let wanted: &[&str] = match (ev, silent) {
                (_, true) => &[],
                (guild_event::MOTD, _) => &["motd:a", "roster:stale"],
                _ => &["roster:stale"],
            };
            assert_eq!(words, wanted, "event {ev:#04x}");
        }
        // The rename: no cached record, no signal (`0x5e72e4`); with one, the rank is renamed
        // and the signal follows.
        let mut guild = GuildState::default();
        guild.mirror_self(7, 0);
        guild.apply_event(&event(
            guild_event::UPDATE_RANK_NAME,
            &["1", "Officer"],
            None,
        ));
        assert!(guild.lane.take_words().is_empty(), "no record");
        guild
            .identities
            .insert(7, identity("Legacy", &["GM", "Off"]));
        guild.apply_event(&event(
            guild_event::UPDATE_RANK_NAME,
            &["1", "Officer"],
            None,
        ));
        assert_eq!(guild.lane.take_words(), ["roster:stale"]);
        // No player (the mirror holds no guild): no signal either.
        let mut guild = GuildState::default();
        guild.identities.insert(7, identity("Legacy", &["GM"]));
        guild.apply_event(&event(guild_event::UPDATE_RANK_NAME, &["0", "X"], None));
        assert!(guild.lane.take_words().is_empty(), "no player");
    }

    #[test]
    fn leaving_the_guild_leaves_the_roster_and_the_identities() {
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
        assert_eq!(
            guild.now.roster.members.len(),
            1,
            "only `InitGuildRoster` and the roster message write the counts (`0x4d0a13`, `0x4d0ae2`)"
        );
        assert_eq!(guild.now.roster.motd, "Raid at eight");
        assert!(guild.identities.answered(7), "identities survive");

        assert!(!guild.mirror_self(0, 0), "no edge when nothing moved");
    }

    /// At login the MOTD event arrives before our descriptor names the guild.
    #[test]
    fn the_login_motd_event_signals_before_the_descriptor_names_the_guild() {
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
        assert!(guild.now.roster.motd.is_empty(), "the event stores nothing");
        assert_eq!(
            guild.lane.take_words(),
            ["motd:Raid Wednesday at eight.", "roster:stale"],
            "the mirror signals nothing of its own: the roster update is the server's event"
        );
    }

    #[test]
    fn rights_index_through_the_rank_array() {
        let mut guild = GuildState::default();
        assert_eq!(guild.now.roster.rights(1), 0, "no roster yet");
        guild.apply_roster(GuildRoster {
            rank_rights: vec![0xffff, 0x00ff, 0x000f],
            ..Default::default()
        });
        assert_eq!(guild.now.roster.rights(1), 0x00ff);
        assert_eq!(guild.now.roster.rank_rights.len(), 3);
        assert_eq!(guild.now.roster.rights(9), 0, "a rank past the array");
    }

    #[test]
    fn an_over_long_rank_array_is_clamped() {
        let mut guild = GuildState::default();
        guild.apply_roster(GuildRoster {
            rank_rights: vec![1; 14],
            ..Default::default()
        });
        assert_eq!(guild.now.roster.rank_rights.len(), GUILD_RANKS_MAX_COUNT);
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
        assert_eq!(
            guild.now.identities[&7].rank_name(1),
            "Officer",
            "and the version the getters read"
        );
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
        let at = |local: &GuildLocal, index: i64| {
            let rows = local.ordered(&guild.now.roster, true, None, Default::default(), &|_| {});
            live::row_at(&rows, index).map(|r| r.info.name.clone())
        };
        local.sort.select(sort::SortField::Name);
        assert_eq!(at(&local, 2).as_deref(), Some("Bob"), "name, ascending");
        local.sort.select(sort::SortField::Name);
        assert_eq!(at(&local, 2).as_deref(), Some("Alice"), "and reversed");
        assert!(at(&local, 0).is_none());
        assert!(at(&local, 3).is_none());
    }

    /// A loop over the roster's rows sorts it once: the order is built again only when the list,
    /// the chain or show-offline moves.
    #[test]
    fn the_display_order_is_built_once_until_something_it_depends_on_moves() {
        let mut guild = GuildState::default();
        let local = &mut GuildLocal::default();
        let roster = |names: &[&str]| GuildRoster {
            members: names
                .iter()
                .enumerate()
                .map(|(i, n)| member(i as u64 + 1, n, guild_presence::ONLINE))
                .collect(),
            ..Default::default()
        };
        guild.apply_roster(roster(&["Alder", "Birch"]));
        let order = |guild: &GuildState, local: &GuildLocal, show_offline| {
            local.ordered(
                &guild.now.roster,
                show_offline,
                None,
                Default::default(),
                &|_| {},
            )
        };
        let first = order(&guild, local, false);
        assert!(
            Arc::ptr_eq(&first, &order(&guild, local, false)),
            "the same list"
        );
        assert!(
            !Arc::ptr_eq(&first, &order(&guild, local, true)),
            "show-offline moved"
        );
        let first = order(&guild, local, false);
        local.sort.select(SortField::Name);
        assert!(
            !Arc::ptr_eq(&first, &order(&guild, local, false)),
            "the chain moved"
        );
        let second = order(&guild, local, false);
        guild.apply_event(&event(guild_event::MOTD, &["hi"], None));
        assert!(
            Arc::ptr_eq(&second, &order(&guild, local, false)),
            "a message of the day leaves the list alone"
        );
        guild.apply_roster(roster(&["Alder", "Birch", "Cedar"]));
        let third = order(&guild, local, false);
        assert!(!Arc::ptr_eq(&second, &third), "a new list");
        assert_eq!(third.len(), 3);
    }

    #[test]
    fn each_invitation_raises_its_popup() {
        let mut guild = GuildState::default();
        for (inviter, name) in [("Tigole", "Legacy of Steel"), ("Furor", "Fury")] {
            net::invite(
                &mut guild,
                &crate::ui_action::UiErrorKeys::default(),
                inviter.into(),
                name.into(),
            );
        }
        assert_eq!(
            guild.lane.take_words(),
            ["invite:Tigole:Legacy of Steel", "invite:Furor:Fury"],
            "a second invitation fires again"
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

    /// `PLAYER_GUILD_UPDATE` is the descriptor watchers' (`crate::field_watch`, `0x5e2770`,
    /// `0x5e27b0`), and the reference's roster update comes with the server's own guild event: the
    /// mirror of our guild id and rank signals nothing on either edge.
    #[test]
    fn the_descriptor_mirror_signals_nothing() {
        let mut guild = GuildState::default();
        for (id, rank) in [(7, 0), (7, 3), (9, 3), (0, 0)] {
            guild.mirror_self(id, rank);
            assert_eq!(guild.guild_id, id);
            assert!(guild.lane.take_words().is_empty(), "{id}/{rank}");
        }
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
