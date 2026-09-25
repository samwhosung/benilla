//! The guild session: the identity cache, the roster, the ranks and the outbound verbs.
//! [`GuildState`] mirrors the guild packets and [`feed`] turns it into the snapshot the guild
//! windows read. The roster carries no guild or rank names: those come only from
//! `SMSG_GUILD_QUERY_RESPONSE`, cached by guild id and filled lazily, as in the reference.

use benilla_formats::GuildEmblem;
use benilla_protocol::messages::{
    guild_event, GuildCommandResult, GuildEventNotice, GuildInfo, GuildQueryResponse, GuildRoster,
    GuildRosterMember, GUILD_RANKS_MAX_COUNT,
};
use benilla_protocol::ObjectFields;
use benilla_ui::script::{LastOnline, UnitGuild};
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::QueryCache;
use crate::ui_script::{UiFeed, UiInput};

mod feed;
mod lines;
mod sort;

pub(crate) use sort::RosterRow;
use sort::{SortField, SortStack};

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

    fn exists(&self) -> bool {
        !self.name.is_empty()
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

/// The guild session mirror: filled by the packet handlers, read by the feed, reset on disconnect.
#[derive(Resource, Default)]
pub(crate) struct GuildState {
    /// Our `PLAYER_GUILDID` (field 191), `0` when guildless; `IsInGuild` reads it, not the roster.
    guild_id: u32,
    /// Our `PLAYER_GUILDRANK` (field 192), 0-based, `0` = guild master.
    rank_index: u32,
    /// Guild id to identity, asked once, negatives included; keyed by id because `PLAYER_GUILDID`
    /// is public, so any visible player's guild is nameable ([`unit_guild`]).
    identities: QueryCache<u32, Identity>,
    /// Bumped when an identity lands or is evicted, never by an ask; the unit feeds watch it.
    identity_generation: u64,
    /// Apart from the roster: `GE_MOTD` sets it alone, and at login vmangos sends that event before
    /// any roster (`CharacterHandler.cpp:558`).
    motd: String,
    /// The guild information text (`GuildInfoFrame`), carried only by the roster.
    info_text: String,
    /// One rights word per rank id; its length is the guild's real rank count.
    rank_rights: Vec<u32>,
    /// The members, in wire order.
    members: Vec<GuildRosterMember>,
    sort: SortStack,
    /// `SetGuildRosterShowOffline`, off at start like the reference's `0xb73124` (`0x4d0a25`); the
    /// FrameXML restores the saved value at `VARIABLES_LOADED`.
    show_offline: bool,
    /// The selected member as a guid, `0` for none, as `SetGuildRosterSelection 0x4d1820` stores
    /// it (`0xb73128`, at `0x4d186a`/`0x4d1872`): a re-sort keeps the same player selected.
    selection: u64,
    /// Member guids in the order last shown, mapping a Lua row index back to its player.
    display_order: Vec<u64>,
    /// The held invitation, `(inviter, guild)`; the accept and decline packets do not name it.
    pending_invite: Option<(String, String)>,
    roster_event: Option<RosterUpdate>,
    /// Real-time seconds before which `GuildRoster()` is swallowed; `0.0` = allowed now.
    roster_allowed_at: f64,
    /// The pushed snapshot is stale; the feed rebuilds only when set.
    dirty: bool,
    /// `/ginfo` answers for [`feed`], which builds their lines where the VM is: their templates are
    /// not catalog rows (`0x5e6fb0`), so they cannot ride [`crate::ui_action::UiErrorKeys`].
    pending_info: Vec<GuildInfo>,
}

impl crate::query_cache::AskOnce for GuildState {
    fn clear_pending(&mut self) {
        self.identities.clear_pending();
    }
}

impl GuildState {
    /// `IsInGuild`: our own `PLAYER_GUILDID` is set, known before any guild packet.
    pub(crate) fn in_guild(&self) -> bool {
        self.guild_id != 0
    }

    /// Mirror our descriptor's guild fields; `true` when either moved. Asks for nothing: the
    /// identity cache is lazy and the roster is the FrameXML's to request.
    fn mirror_self(&mut self, guild_id: u32, rank_index: u32) -> bool {
        if (self.guild_id, self.rank_index) == (guild_id, rank_index) {
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
            self.selection = 0;
        }
        if moved {
            self.note_roster_update(RosterUpdate::Applied);
        }
        self.dirty = true;
        true
    }

    /// Owe the VM a `GUILD_ROSTER_UPDATE`. [`RosterUpdate::Stale`] wins over `Applied` in one
    /// frame, since a re-request also repaints, and clears the request throttle.
    fn note_roster_update(&mut self, kind: RosterUpdate) {
        if kind.arg1() {
            self.roster_allowed_at = 0.0;
        }
        if kind.arg1() || self.roster_event.is_none() {
            self.roster_event = Some(kind);
        }
        self.dirty = true;
    }

    /// Send `CMSG_GUILD_QUERY` for a guild we hold nothing for, a negative counting as held.
    fn request_identity(&self, guild_id: u32, commands: &NetCommands) {
        if guild_id != 0 {
            self.identities.get_or_ask(guild_id, || {
                let _ = commands.0.send(ClientCommand::GuildQuery { guild_id });
            });
        }
    }

    /// A guild's identity if we hold a real one; `None` while in flight and for no such guild.
    fn identity(&self, guild_id: u32) -> Option<&Identity> {
        self.identities.get(guild_id).filter(|i| i.exists())
    }

    /// The read that also asks: `None` on a miss, which sends the query, as the reference's
    /// `GetGuildRosterInfo` answers nil and queries inline (`0x4d1291`).
    fn resolve_identity(&self, guild_id: u32, commands: &NetCommands) -> Option<&Identity> {
        self.request_identity(guild_id, commands);
        self.identity(guild_id)
    }

    /// The landed-identity counter the unit feeds watch.
    pub(crate) fn identity_generation(&self) -> u64 {
        self.identity_generation
    }

    /// Our guild's five tabard fields for the tabard designer, `-1`s when undesigned as the wire
    /// carries them; a miss sends the query.
    pub(crate) fn own_emblem_record(
        &self,
        guild_id: u32,
        commands: &NetCommands,
    ) -> Option<[i32; 5]> {
        let e = self.resolve_identity(guild_id, commands)?.emblem;
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
        if self.identities.evict(self.guild_id) {
            self.identity_generation = self.identity_generation.wrapping_add(1);
            self.dirty = true;
        }
    }

    /// `SMSG_GUILD_QUERY_RESPONSE`: fill the identity cache, an empty name as a negative.
    fn apply_query_response(&mut self, response: GuildQueryResponse) {
        let ours = response.guild_id == self.guild_id;
        self.identities.insert(
            response.guild_id,
            Some(Identity {
                name: response.name,
                rank_names: response.rank_names,
                emblem: GuildEmblem {
                    emblem_style: response.emblem_style,
                    emblem_color: response.emblem_color,
                    border_style: response.border_style,
                    border_color: response.border_color,
                    background_color: response.background_color,
                },
            }),
        );
        self.identity_generation = self.identity_generation.wrapping_add(1);
        self.dirty = true;
        if ours {
            // Our rank names landed: repaint, never re-request, as the reference's cache callback
            // `0x4d1480` does (`0x4d148c`).
            self.note_roster_update(RosterUpdate::Applied);
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

    /// `SMSG_GUILD_INVITE`: hold the invitation, which fires the popup's show edge.
    fn apply_invite(&mut self, inviter: String, guild: String) {
        self.pending_invite = Some((inviter, guild));
        self.dirty = true;
    }

    /// The popup's Accept or Decline. Nothing raises a hide edge (`0x48f470` has no callers), but
    /// clearing lets a second invitation fire the show edge again.
    fn clear_invite(&mut self) {
        if self.pending_invite.take().is_some() {
            self.dirty = true;
        }
    }

    /// `SortGuildRoster(field)`: re-order and repaint, never re-request ([`sort::SortStack`]).
    fn sort_by(&mut self, field: &str) {
        self.sort.select(SortField::parse(field));
        self.note_roster_update(RosterUpdate::Applied);
    }

    /// `SetGuildRosterShowOffline(flag)`: re-sort and repaint; an unchanged value does nothing,
    /// as in the reference (`0x4d0f70`).
    fn set_show_offline(&mut self, on: bool) {
        if self.show_offline != on {
            self.show_offline = on;
            self.note_roster_update(RosterUpdate::Applied);
        }
    }

    /// `SetGuildRosterSelection(index)`: store the row's guid. No event: the stock click handler
    /// repaints itself (`FriendsFrame.lua:706`).
    fn select(&mut self, index: u32) {
        let guid = self.guid_at(index).unwrap_or(0);
        if self.selection != guid {
            self.selection = guid;
            self.dirty = true;
        }
    }

    /// The guid at a 1-based display row, over the whole roster, not the show-offline count.
    fn guid_at(&self, index: u32) -> Option<u64> {
        index
            .checked_sub(1)
            .and_then(|i| self.display_order.get(i as usize))
            .copied()
    }

    /// The member at a 1-based display row, mutable because the note verbs write before they send.
    fn member_at_mut(&mut self, index: u32) -> Option<&mut GuildRosterMember> {
        let guid = self.guid_at(index)?;
        self.members.iter_mut().find(|m| m.guid == guid)
    }

    /// `GetNumGuildMembers()`: the whole roster with show-offline on, else the online tally
    /// (`0x4d1190`, against `0xb73118`/`0xb7311c`). A loop bound, not the addressable range: with
    /// show-offline off the offline rows sort last ([`sort`]).
    fn num_members(&self) -> usize {
        if self.show_offline {
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

/// `GetGuildInfo(unit)`: the unit's public guild fields joined against the identity cache, asking
/// on a miss. `None` for a guildless unit, a creature or an unanswered query, one nil path in the
/// reference (`0x4c93d7`, `0x4c943c`).
pub(crate) fn unit_guild(
    fields: &ObjectFields,
    guild: &mut GuildState,
    commands: &NetCommands,
) -> Option<UnitGuild> {
    let guild_id = fields.player_guild_id();
    let rank_index = fields.player_guild_rank();
    let identity = guild.resolve_identity(guild_id, commands)?;
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
    guild: &'a mut GuildState,
    commands: &NetCommands,
) -> Option<&'a str> {
    guild
        .resolve_identity(fields.player_guild_id(), commands)
        .map(|identity| identity.name.as_str())
}

/// A unit's guild crest for the body composite, off its public `PLAYER_GUILDID`, asking on a miss
/// like [`unit_guild`]. `None` keeps a Guild Tabard's own `Tabard_A_05Default` art, as the
/// reference never enters its install (`0x47a610`) for a guildless wearer (`0x560e30` returns NULL
/// at `0x560e3f`), a creature, a query in flight (re-dressed when
/// [`GuildState::identity_generation`] moves; the reference's callback is `0x5e0650`) or an
/// undesigned `-1` crest, which would paint blank.
pub(crate) fn unit_guild_emblem(
    fields: &ObjectFields,
    guild: &mut GuildState,
    commands: &NetCommands,
) -> Option<GuildEmblem> {
    guild_emblem(fields.player_guild_id(), guild, commands)
}

/// The same crest for a corpse, off its own `CORPSE_FIELD_GUILD` (`0x5d6edf`) through the same
/// lookup (`0x5d6ec0`), with the same `None` cases.
pub(crate) fn corpse_guild_emblem(
    fields: &ObjectFields,
    guild: &mut GuildState,
    commands: &NetCommands,
) -> Option<GuildEmblem> {
    guild_emblem(fields.corpse_guild(), guild, commands)
}

fn guild_emblem(
    guild_id: u32,
    guild: &mut GuildState,
    commands: &NetCommands,
) -> Option<GuildEmblem> {
    let emblem = guild.resolve_identity(guild_id, commands)?.emblem;
    emblem.is_designed().then_some(emblem)
}

/// The guild packet handlers: they update the mirror and queue the message ids [`lines`] names,
/// whose catalog rows pick the surface and the sound; the guild events fire from the feed.
pub(crate) mod net {
    use super::*;
    use crate::ui_action::{UiError, UiErrorKeys};
    use crate::ui_social::SocialState;
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

    fn on_query_response(In(ev): In<SessionEvent>, mut guild: ResMut<GuildState>) {
        if let SessionEvent::GuildQueryResponse(response) = ev {
            query_response(&mut guild, response);
        }
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
        mut errors: ResMut<UiErrorKeys>,
        social: Res<SocialState>,
        notify: Res<GuildMemberNotify>,
        self_guid: Res<crate::net::SelfGuid>,
    ) {
        if let SessionEvent::GuildEvent(notice) = ev {
            event(
                &mut guild,
                &mut errors,
                &social,
                &notify,
                self_guid.0,
                notice,
            );
        }
    }

    fn on_command_result(
        In(ev): In<SessionEvent>,
        mut guild: ResMut<GuildState>,
        mut errors: ResMut<UiErrorKeys>,
    ) {
        if let SessionEvent::GuildCommandResult(result) = ev {
            command_result(&mut guild, &mut errors, result);
        }
    }

    fn on_invite(
        In(ev): In<SessionEvent>,
        mut guild: ResMut<GuildState>,
        mut errors: ResMut<UiErrorKeys>,
    ) {
        if let SessionEvent::GuildInvite { inviter, guild: g } = ev {
            invite(&mut guild, &mut errors, inviter, g);
        }
    }

    fn on_decline(In(ev): In<SessionEvent>, mut errors: ResMut<UiErrorKeys>) {
        if let SessionEvent::GuildDecline { name } = ev {
            decline(&mut errors, &name);
        }
    }

    fn on_info(In(ev): In<SessionEvent>, mut guild: ResMut<GuildState>) {
        if let SessionEvent::GuildInfo(i) = ev {
            info(&mut guild, i);
        }
    }

    /// Reset at session end: the next login may be another character. Deviation: the identity
    /// cache goes too, where the reference keeps it in `guildcache.wdb`, because a renamed guild
    /// would otherwise show its old name.
    fn on_session_end(In(_): In<SessionEvent>, mut guild: ResMut<GuildState>) {
        *guild = GuildState::default();
    }

    /// Queue the message ids [`lines`] named.
    fn push_lines(errors: &mut UiErrorKeys, lines: impl IntoIterator<Item = UiError>) {
        errors.0.extend(lines);
    }

    /// `SMSG_GUILD_QUERY_RESPONSE`.
    pub(crate) fn query_response(guild: &mut GuildState, response: GuildQueryResponse) {
        guild.apply_query_response(response);
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
        errors: &mut UiErrorKeys,
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
        errors: &mut UiErrorKeys,
        result: GuildCommandResult,
    ) {
        guild.apply_command_result(&result);
        push_lines(errors, lines::command_line(&result));
    }

    /// `SMSG_GUILD_INVITE`: the popup's show edge and the notice line printed beside it.
    pub(crate) fn invite(
        guild: &mut GuildState,
        errors: &mut UiErrorKeys,
        inviter: String,
        guild_name: String,
    ) {
        push_lines(errors, [lines::invite_line(&inviter, &guild_name)]);
        guild.apply_invite(inviter, guild_name);
    }

    /// `SMSG_GUILD_DECLINE`: a line, no state.
    pub(crate) fn decline(errors: &mut UiErrorKeys, name: &str) {
        push_lines(errors, [lines::decline_line(name)]);
    }

    /// `SMSG_GUILD_INFO`, the `/ginfo` answer: parked for the feed, which builds its two lines
    /// ([`GuildState::pending_info`]).
    pub(crate) fn info(guild: &mut GuildState, info: GuildInfo) {
        guild.pending_info.push(info);
    }
}

/// The guild feed's system set, so the chat drain can run after the guild events fire.
#[derive(bevy::ecs::schedule::SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct GuildFeed;

/// The guild windows' session: the wire mirror, the VM feed, and the outbound intents.
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
            .init_resource::<GuildMemberNotify>()
            .add_systems(
                Update,
                (
                    feed::feed_guild.in_set(UiFeed).in_set(GuildFeed),
                    feed::drain_guild.after(UiInput),
                )
                    // Never against the boot VM: the feed's edge memo survives the entry load, so
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
        );
        crate::ui_social::net::ignore_list(&mut social, vec![9]);
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

        let rows = feed::display_rows(&guild, None, None);
        assert_eq!(rows.len(), 3, "every member, always");
        assert_eq!(guild.num_members(), 2, "but the count is the online tally");
        assert!(
            rows[..2].iter().all(|r| r.info.online),
            "and the counted rows are the leading ones"
        );
        assert!(!rows[2].info.online);

        guild.set_show_offline(true);
        let rows = feed::display_rows(&guild, None, None);
        assert_eq!(rows.len(), 3);
        assert_eq!(guild.num_members(), 3, "now the count is everybody");
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
    fn an_empty_query_name_is_a_negative_not_a_blank_guild() {
        let (commands, rx) = net_commands();
        let mut guild = GuildState::default();

        assert!(guild.resolve_identity(7, &commands).is_none());
        assert!(guild.resolve_identity(7, &commands).is_none());
        assert_eq!(rx.try_iter().count(), 1, "asked exactly once");

        guild.apply_query_response(GuildQueryResponse {
            guild_id: 7,
            name: String::new(),
            ..Default::default()
        });
        assert!(guild.identities.answered(7), "cached as a negative");
        assert!(!guild.identities.is_pending(7), "and no longer in flight");
        assert!(
            guild.resolve_identity(7, &commands).is_none(),
            "an empty name is not a guild"
        );
        assert_eq!(rx.try_iter().count(), 0, "and is never re-asked");
    }

    #[test]
    fn a_guild_event_asks_again_and_a_local_resort_does_not() {
        let mut guild = GuildState::default();

        guild.apply_event(&event(guild_event::JOINED, &["Furor"], None));
        assert_eq!(guild.roster_event.take(), Some(RosterUpdate::Stale));
        assert!(RosterUpdate::Stale.arg1(), "the pane re-requests");

        guild.sort_by("level");
        assert_eq!(guild.roster_event.take(), Some(RosterUpdate::Applied));
        assert!(
            !RosterUpdate::Applied.arg1(),
            "a column click must never ask the server for a roster"
        );

        guild.set_show_offline(true);
        assert_eq!(guild.roster_event.take(), Some(RosterUpdate::Applied));

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
    fn the_stale_edge_clears_the_request_throttle() {
        let mut guild = GuildState {
            roster_allowed_at: 12_345.0,
            ..Default::default()
        };
        guild.note_roster_update(RosterUpdate::Applied);
        assert_eq!(guild.roster_allowed_at, 12_345.0, "a repaint does not");
        guild.note_roster_update(RosterUpdate::Stale);
        assert_eq!(guild.roster_allowed_at, 0.0);
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
        guild
            .identities
            .insert(7, Some(identity("Legacy", &["GM"])));
        assert!(guild.mirror_self(7, 0), "joined");
        guild.apply_roster(GuildRoster {
            motd: "Raid at eight".into(),
            rank_rights: vec![0xffff, 0x3],
            members: vec![member(1, "Tigole", guild_presence::ONLINE)],
            ..Default::default()
        });
        guild.selection = 1;
        assert!(guild.in_guild());

        assert!(guild.mirror_self(0, 0), "left");
        assert!(!guild.in_guild());
        assert!(guild.members.is_empty());
        assert!(guild.motd.is_empty());
        assert_eq!(guild.selection, 0);
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
            .insert(7, Some(identity("Legacy", &["GM", "Off"])));
        guild.apply_event(&event(
            guild_event::UPDATE_RANK_NAME,
            &["1", "Officer"],
            None,
        ));
        assert_eq!(guild.identities.get(7).unwrap().rank_name(1), "Officer");
        assert!(guild.dirty);
    }

    #[test]
    fn selection_follows_the_player_not_the_row() {
        let mut guild = GuildState::default();
        guild.apply_roster(GuildRoster {
            members: vec![
                member(11, "Alice", guild_presence::ONLINE),
                member(22, "Bob", guild_presence::ONLINE),
            ],
            ..Default::default()
        });
        guild.display_order = vec![11, 22];
        guild.select(2);
        assert_eq!(guild.selection, 22);
        assert_eq!(
            guild.member_at_mut(2).map(|m| m.name.clone()),
            Some("Bob".to_string()),
            "a display row resolves to the member the wire wants by name"
        );

        // Re-ordered: the same player stays selected.
        guild.display_order = vec![22, 11];
        assert_eq!(feed::index_of(&guild.display_order, guild.selection), 1);

        guild.select(0);
        assert_eq!(guild.selection, 0, "0 = nothing selected");
        guild.select(9);
        assert_eq!(guild.selection, 0, "past the end selects nothing");

        // Bob leaves: the stored guid stops resolving.
        guild.selection = 22;
        guild.apply_roster(GuildRoster {
            members: vec![member(11, "Alice", guild_presence::ONLINE)],
            ..Default::default()
        });
        guild.display_order = vec![11];
        assert_eq!(feed::index_of(&guild.display_order, guild.selection), 0);
    }

    #[test]
    fn an_invitation_arms_and_answering_clears_it() {
        let mut guild = GuildState::default();
        guild.apply_invite("Tigole".into(), "Legacy of Steel".into());
        assert_eq!(
            guild.pending_invite,
            Some(("Tigole".into(), "Legacy of Steel".into()))
        );
        guild.clear_invite();
        assert_eq!(guild.pending_invite, None);
    }

    #[test]
    fn unit_guild_asks_on_a_miss_and_never_names_a_blank_guild() {
        let (commands, rx) = net_commands();
        let mut guild = GuildState::default();
        guild
            .identities
            .insert(7, Some(identity("Legacy", &["GM", "Off"])));

        // A creature or a guildless player: no `PLAYER_GUILDID`.
        let guildless = ObjectFields::from_pairs(&[]);
        assert!(unit_guild(&guildless, &mut guild, &commands).is_none());
        assert_eq!(rx.try_iter().count(), 0, "guild id 0 asks nothing");

        // Fields 191/192 = PLAYER_GUILDID/PLAYER_GUILDRANK.
        let unknown = ObjectFields::from_pairs(&[(191, 9), (192, 1)]);
        assert!(
            unit_guild(&unknown, &mut guild, &commands).is_none(),
            "a query in flight is not a blank-named guild"
        );
        assert_eq!(rx.try_iter().count(), 1, "and the miss asked for it");

        let member = ObjectFields::from_pairs(&[(191, 7), (192, 1)]);
        let resolved = unit_guild(&member, &mut guild, &commands).expect("cached");
        assert_eq!(resolved.name, "Legacy");
        assert_eq!(resolved.rank_name, "Off");
        assert_eq!(resolved.rank_index, 1);

        // A rank past the ten slots names nothing.
        let odd_rank = ObjectFields::from_pairs(&[(191, 7), (192, 99)]);
        assert_eq!(
            unit_guild(&odd_rank, &mut guild, &commands)
                .unwrap()
                .rank_name,
            ""
        );
    }

    #[test]
    fn unit_guild_name_is_unit_guilds_name_on_every_leg() {
        let (commands, rx) = net_commands();
        let mut guild = GuildState::default();
        guild
            .identities
            .insert(7, Some(identity("Legacy", &["GM", "Off"])));
        // The negative cache: an empty answer is no such guild.
        guild.identities.insert(8, Some(identity("", &[])));

        let guildless = ObjectFields::from_pairs(&[]);
        assert_eq!(unit_guild_name(&guildless, &mut guild, &commands), None);
        assert_eq!(rx.try_iter().count(), 0, "guild id 0 asks nothing");

        let unknown = ObjectFields::from_pairs(&[(191, 9)]);
        assert_eq!(
            unit_guild_name(&unknown, &mut guild, &commands),
            None,
            "a query in flight draws no line"
        );
        assert_eq!(rx.try_iter().count(), 1, "and the miss asked for it");

        let blank = ObjectFields::from_pairs(&[(191, 8)]);
        assert_eq!(unit_guild_name(&blank, &mut guild, &commands), None);
        assert_eq!(
            rx.try_iter().count(),
            0,
            "the negative cache re-asks nothing"
        );

        let member = ObjectFields::from_pairs(&[(191, 7), (192, 1)]);
        assert_eq!(
            unit_guild_name(&member, &mut guild, &commands),
            Some("Legacy")
        );
        let via_line = unit_guild_name(&member, &mut guild, &commands).map(str::to_owned);
        let via_api = unit_guild(&member, &mut guild, &commands).map(|g| g.name);
        assert_eq!(via_line, via_api, "the two readers of one cache disagree");
    }

    /// A command channel whose receiver stays alive, so a send neither blocks nor is dropped.
    fn net_commands() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }
}
