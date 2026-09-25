//! The guild session — the identity cache, the roster, the ranks, and the outbound verbs.
//!
//! [`GuildState`] mirrors the wire the way [`crate::ui_social`]'s `SocialState` does: the seven
//! server packets replace it or patch it, and the feed ([`feed`]) turns it into the display-ready
//! snapshot `benilla_ui::script::guild` reads. The laws below are the ones that are *not* what the
//! obvious design would do, each as the reference does it:
//!
//! - **Identity and roster are two caches with two lifetimes.** `SMSG_GUILD_ROSTER` carries the
//!   MOTD, the info text, the rank *rights* and the members; it carries neither the guild's name
//!   nor the rank *names*. Those come only from `SMSG_GUILD_QUERY_RESPONSE`, keyed by guild id.
//!   [`GuildState::identities`] is that second cache, keyed by id precisely because
//!   `PLAYER_GUILDID` (191) is PUBLIC: every visible player's guild has to be nameable, not just
//!   ours ([`unit_guild`]).
//! - **That cache is LAZY, and nothing primes it.** No query goes out at world entry. The first
//!   read that needs an uncached guild fires `CMSG_GUILD_QUERY` **and answers empty for that
//!   call** — `GetGuildRosterInfo` does it inline (`0x4d1291`, returning its whole nil tuple on
//!   the miss) and the arrival fires `GUILD_ROSTER_UPDATE` with no args (`0x4d1480`). Ours is
//!   [`GuildState::resolve_identity`], shaped like [`crate::names::NameCache::resolve`], which is
//!   this house's existing name for that idiom.
//! - **An empty name in the query response is "no such guild"**, not a guild with a blank name —
//!   the reference tests exactly that (`0x5552ae test al,al`, choosing the cache *insert*
//!   `0x561070` or the cache *remove* `0x561390`). Cached as a negative so the query is not
//!   re-sent forever.
//! - **The roster is sorted, never filtered.** Show-offline changes the *count*
//!   ([`GuildState::num_members`]), never the array. See [`sort`], and see
//!   `benilla_ui::script::GuildState::num_members` for why collapsing the two is wrong.
//! - **Selection is a guid**, not a row: `SetGuildRosterSelection 0x4d1820` stores the member's
//!   guid (`0x4d186a`/`0x4d1872`) and `GetGuildRosterSelection 0x4d1890` linear-searches it back
//!   to a 1-based index, so a re-sort keeps the same *player* highlighted and a member who leaves
//!   silently becomes "nothing selected" — exactly the friend list's `+0x648` shape.
//! - **`GuildRoster()` is throttled to one request per 10 s** (`0x4d10d0` against `0xb73130`), and
//!   the one branch that sets `GUILD_ROSTER_UPDATE`'s arg1 clears that throttle first
//!   (`0x4d1160`). See [`RosterUpdate`]: the two halves are one mechanism, and implementing either
//!   without the other breaks it.

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

/// How long `GuildRoster()` is silenced for after a request goes out — the reference's own
/// 10 000 ms against `0xb73130` (`0x4d10d0`), which is what stops a pane that re-requests on every
/// guild event from hammering the server. Cleared, not merely bypassed, by the arg1 branch of
/// [`RosterUpdate`].
const ROSTER_REQUEST_THROTTLE_SECS: f64 = 10.0;

/// One guild's public identity — everything `SMSG_GUILD_QUERY_RESPONSE` carries that we keep.
#[derive(Clone, Debug, Default)]
struct Identity {
    /// The guild's name — **empty is the negative cache**: "this id names no guild" (module doc).
    name: String,
    /// The ten rank names, index 0 = guild master; empty past the guild's real rank count.
    rank_names: [String; GUILD_RANKS_MAX_COUNT],
    /// The guild's tabard — the five emblem indices, which are what the **body composite** paints
    /// onto a member's guild tabard. This is the second consumer of the identity
    /// cache and the reason it is not a UI-only structure: the reference reads the same cached
    /// record from the character compositor (`0x6d6d20` → `0x47a610`) and from `GetGuildInfo`.
    emblem: GuildEmblem,
}

impl Identity {
    /// The name of rank `index` (0-based), or `""` — bounded, unlike the reference's own
    /// `shl edx,0x6` at `0x4c93e7`, which happily indexes past its ten slots.
    fn rank_name(&self, index: u32) -> &str {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.rank_names.get(i))
            .map(String::as_str)
            .unwrap_or_default()
    }

    /// Is this a real guild, or the "no such guild" negative? See the module doc.
    fn exists(&self) -> bool {
        !self.name.is_empty()
    }
}

/// Which `GUILD_ROSTER_UPDATE` the feed owes the VM — **and this is the whole of that decision**.
///
/// `arg1` is a control signal, not data: `FriendsFrame_OnEvent`'s arm is
/// `if ( arg1 ) then GuildRoster() end` before its repaint (`FriendsFrame.lua:646-653`), so a
/// truthy arg1 means "the roster you hold is stale, ask the server for a fresh one". The engine
/// side is one function, `0x4d1160`, and the complete caller census is:
///
/// | arg1 | site | meaning |
/// |---|---|---|
/// | — | `0x4d0d3c` | a fresh `SMSG_GUILD_ROSTER` was parsed |
/// | — | `0x4d0f9f` | show-offline changed and the view was re-sorted |
/// | — | `0x4d1022` | `SortGuildRoster` re-sorted the view |
/// | — | `0x4d148c` | the guild-name cache record arrived |
/// | `1` | `0x5e74c4` | `SMSG_GUILD_EVENT`, cases `0x00`–`0x0b` **only** |
/// | `1` | `0x5e7792` | `SMSG_GUILD_COMMAND_RESULT`, its two arg-bearing arms |
///
/// Two things in that census are easy to get wrong. **`SIGNED_ON`/`SIGNED_OFF` (`0x0c`/`0x0d`) do
/// NOT set arg1** — a guildmate logging in or out does not force a re-request. And the arg-bearing
/// branch **clears the `GuildRoster()` throttle** (`mov dword ptr [0xb73130],0`) before firing, so
/// that the `GuildRoster()` the FrameXML issues in response cannot be the one the 10 s limiter
/// swallows; [`GuildState::note_roster_update`] does the same, and without it the arg1 path would
/// be a no-op nine times out of ten.
///
/// Getting the flag itself backwards fails in both directions: arg1 on the *applied* edge is an
/// infinite request loop (roster → update(1) → `GuildRoster()` → roster → …), and no arg1 on the
/// *stale* edge is a pane that never refreshes. The reference cannot loop for exactly this reason
/// — the roster's own arrival fires the no-arg form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RosterUpdate {
    /// Redraw from what you have: a roster we just applied, a local re-order, a late identity.
    Applied,
    /// The cached roster is stale — re-request it.
    Stale,
}

impl RosterUpdate {
    /// Does this edge carry arg1? The one place the answer lives.
    fn arg1(self) -> bool {
        self == RosterUpdate::Stale
    }
}

/// **Guild Member Alert** — 1.12's `guildMemberNotify`, the Chat options page's row (decision
/// 1589): *"Receive notification when guild members log on/off"* (the CVar's own registered help
/// string, `0x860320`).
///
/// A knob rather than a field on [`GuildState`] because it is a *setting*, not session state:
/// GuildState is cleared on disconnect, and a CVar must not be.
///
/// **Off by default, and that is byte-read, not a guess**: the register site `0x5e24c7` pushes
/// `0x82e570` = `"0"`. A stock 1.12 client prints nothing when a guildmate logs in, and
/// neither do we.
#[derive(Resource, Default)]
pub(crate) struct GuildMemberNotify(pub(crate) bool);

/// The guild session mirror. Filled by the net drain's guild arms, read by the feed, cleared on
/// disconnect beside the other per-login resources.
#[derive(Resource, Default)]
pub(crate) struct GuildState {
    /// Our own `PLAYER_GUILDID` (field 191), mirrored from the descriptor; `0` = guildless. This,
    /// not the roster, is what `IsInGuild` answers — it is true from the moment the descriptor
    /// streams, before any packet has been asked for.
    guild_id: u32,
    /// Our own `PLAYER_GUILDRANK` (field 192), **0-based**, `0` = guild master.
    rank_index: u32,
    /// Guild id → identity, from `SMSG_GUILD_QUERY_RESPONSE`, ask-once through [`QueryCache`].
    /// Holds negatives (module doc).
    identities: QueryCache<u32, Identity>,
    /// Bumped by every landed identity ([`Self::apply_query_response`]) — never by an ask. The
    /// gated unit feeds' watch counter: `unit_guild`'s miss resolves later, and
    /// `is_changed` could not flag the landing while the miss itself took `&mut self` per frame
    /// (2288 made it `&self`; the counter stays as the finer signal).
    identity_generation: u64,
    /// The message of the day. Kept beside the roster rather than inside it because `GE_MOTD`
    /// updates it on its own, and at login vmangos sends that event *before* any roster exists
    /// (`CharacterHandler.cpp:558`).
    motd: String,
    /// The guild information text (the `GuildInfoFrame` body) — roster-only.
    info_text: String,
    /// One rights word per rank, indexed by rank id. **Its length is the guild's real rank
    /// count** — the only field on the wire that says how many of the identity's ten rank-name
    /// slots are live.
    rank_rights: Vec<u32>,
    /// The members, in wire order. Display order is the feed's ([`Self::display_order`]).
    members: Vec<GuildRosterMember>,
    /// The sort chain ([`sort::SortStack`]).
    sort: SortStack,
    /// `SetGuildRosterShowOffline`. Starts off, like the reference's own zeroed `ds:0xb73124`
    /// (`0x4d0a25`); the FrameXML restores the player's saved variable at `VARIABLES_LOADED`.
    /// **It changes the count and the order, never the membership** — see [`sort`].
    show_offline: bool,
    /// The selected member, stored as a **guid** like the reference's `ds:0xb73128`; `0` = none.
    selection: u64,
    /// The member guids in the order the feed last showed them — written by the feed, read by the
    /// drain so a row index from Lua maps back to the same player the user clicked.
    display_order: Vec<u64>,
    /// The invitation we are holding, `(inviter, guild)`. The wire's accept/decline say nothing
    /// about *which* invitation they answer, so this is ours to remember.
    pending_invite: Option<(String, String)>,
    /// The `GUILD_ROSTER_UPDATE` the feed owes, if any ([`RosterUpdate`]).
    roster_event: Option<RosterUpdate>,
    /// Real-time seconds before which a `GuildRoster()` is swallowed
    /// ([`ROSTER_REQUEST_THROTTLE_SECS`]). `0.0` = allowed now.
    roster_allowed_at: f64,
    /// Set whenever the pushed snapshot went stale — the feed rebuilds and pushes on this and
    /// skips the work otherwise (a 500-member roster is not worth re-resolving every frame).
    dirty: bool,
    /// `/ginfo` answers waiting for the VM. Their two lines are `GUILD_NAME_TEMPLATE` and
    /// `GUILD_INFO_TEMPLATE`, which are **not** catalog rows — `0x5e6fb0` resolves each token
    /// through the script VM and emits chat directly, passing no message record — so unlike every
    /// other line in this module they cannot ride [`crate::ui_action::UiErrorKeys`], whose whole
    /// contract is that the catalog names the surface. They wait here for [`feed`] instead, which
    /// is where the VM is.
    pending_info: Vec<GuildInfo>,
}

impl crate::query_cache::AskOnce for GuildState {
    fn clear_pending(&mut self) {
        self.identities.clear_pending();
    }
}

impl GuildState {
    /// Are we in a guild? Our own `PLAYER_GUILDID`, nothing else — answerable before any packet.
    pub(crate) fn in_guild(&self) -> bool {
        self.guild_id != 0
    }

    /// Mirror our own descriptor's guild fields. Returns `true` when either moved, which is the
    /// `PLAYER_GUILD_UPDATE` edge.
    ///
    /// It deliberately asks for **nothing**: the identity cache is lazy (module doc) and the
    /// roster is the FrameXML's to request when it opens the pane.
    fn mirror_self(&mut self, guild_id: u32, rank_index: u32) -> bool {
        if (self.guild_id, self.rank_index) == (guild_id, rank_index) {
            return false;
        }
        // **`0` here is "the descriptor has not told us yet", not "a guild we were in"**.
        // The mirror's id starts at 0 and only moves when our own avatar streams, which at a
        // login is a whole packet burst AFTER the server has already sent the guild's MOTD
        // (`SMSG_GUILD_EVENT 0x02`, vmangos `CharacterHandler.cpp` at the top of the login
        // handler). Reading that first 0 → N as a guild *change* threw the MOTD away as the
        // property of a guild we had left — and since the wipe happens here, at the top of the
        // feed, it landed in the same call that would have fired `GUILD_MOTD` further down, so
        // the login line was destroyed before it could ever be taken. A move OUT of a real
        // guild (N → 0, or N → M) is the edge that genuinely invalidates the mirror.
        let known = self.guild_id != 0;
        let moved = self.guild_id != guild_id;
        self.guild_id = guild_id;
        self.rank_index = rank_index;
        if moved && known {
            // A different guild (or none): everything the old roster said is about a guild we are
            // no longer in. The identity cache survives — it is keyed by id and still true.
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

    /// Note that the VM owes a `GUILD_ROSTER_UPDATE`. [`RosterUpdate::Stale`] wins when both land
    /// in one frame — a re-request repaints too, so the stronger signal subsumes the weaker — and
    /// it **clears the request throttle**, which is half of what that branch is for.
    fn note_roster_update(&mut self, kind: RosterUpdate) {
        if kind.arg1() {
            self.roster_allowed_at = 0.0;
        }
        if kind.arg1() || self.roster_event.is_none() {
            self.roster_event = Some(kind);
        }
        self.dirty = true;
    }

    /// Ask for a guild's identity if we do not already hold one — the lazy cache fill (module
    /// doc). Nothing else in this client ever sends `CMSG_GUILD_QUERY`: no world-entry priming, no
    /// sweep. A negative (`""` name) counts as held, so it is never re-asked.
    fn request_identity(&self, guild_id: u32, commands: &NetCommands) {
        if guild_id != 0 {
            self.identities.get_or_ask(guild_id, || {
                let _ = commands.0.send(ClientCommand::GuildQuery { guild_id });
            });
        }
    }

    /// A guild's identity, if we hold a real one. `None` covers both "not asked yet / in flight"
    /// and "no such guild".
    fn identity(&self, guild_id: u32) -> Option<&Identity> {
        self.identities.get(guild_id).filter(|i| i.exists())
    }

    /// [`Self::request_identity`] then [`Self::identity`] — the read that also asks, shaped like
    /// [`crate::names::NameCache::resolve`]: it answers `None` for *this* call and the answer
    /// arrives later, which is the reference's behaviour and not a gap to paper over.
    fn resolve_identity(&self, guild_id: u32, commands: &NetCommands) -> Option<&Identity> {
        self.request_identity(guild_id, commands);
        self.identity(guild_id)
    }

    /// The landed-identity counter — see the [`Self::identity_generation`] field.
    pub(crate) fn identity_generation(&self) -> u64 {
        self.identity_generation
    }

    /// The tabard designer's view of our guild record: `Some(five)` once the
    /// record is cached — `-1`s for an undesigned tabard, as the wire carries them — and `None`
    /// while it has not arrived or the player has no guild. A miss sends the query, the lazy-cache
    /// idiom every other read of this cache uses.
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

    /// A saved emblem's eviction (`0x5e715f`): our guild's cached record is
    /// dropped so the next query anywhere re-fetches it — the tabards of every member in sight
    /// re-dress off the arrival, as the reference's guild-appearance refresh does.
    pub(crate) fn evict_own_identity(&mut self) {
        if self.identities.evict(self.guild_id) {
            self.identity_generation = self.identity_generation.wrapping_add(1);
            self.dirty = true;
        }
    }

    /// `SMSG_GUILD_QUERY_RESPONSE` — fill (or negatively fill) the identity cache.
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
            // Our own rank names just landed under the roster rows that display them, so the pane
            // has to repaint — but nothing on the server moved, so this must not re-request. The
            // reference fires exactly this, from the cache callback `0x4d1480` (`0x4d148c`).
            self.note_roster_update(RosterUpdate::Applied);
        }
    }

    /// `SMSG_GUILD_ROSTER` — always a complete snapshot, never a delta.
    fn apply_roster(&mut self, roster: GuildRoster) {
        self.motd = roster.motd;
        self.info_text = roster.info;
        // The reference's own rank-rights loop has no bound check and overruns its ten-slot array
        // into the member array's control block on a hostile `rankCount >= 12` (`0x4d0bb0`). We
        // clamp: a memory-safety divergence, deliberately.
        self.rank_rights = roster.rank_rights;
        self.rank_rights.truncate(GUILD_RANKS_MAX_COUNT);
        self.members = roster.members;
        self.note_roster_update(RosterUpdate::Applied);
    }

    /// `SMSG_GUILD_EVENT` — what it does to the mirror. The line it prints is [`lines`]'.
    fn apply_event(&mut self, notice: &GuildEventNotice) {
        match notice.event {
            guild_event::MOTD => {
                // The line is the FrameXML's (`ChatFrame.lua:1338`); ours is the state, so
                // `GetGuildRosterMOTD` agrees with the event before the next roster lands.
                self.motd = notice.params.first().cloned().unwrap_or_default();
                self.dirty = true;
            }
            guild_event::UPDATE_RANK_NAME => {
                // A rank was renamed. The reference writes it straight into its cached record
                // (`0x560e30`) and fires no FrameScript event of its own, so the pushed snapshot
                // is the only thing that can carry it to the pane. Params are `RankID,
                // NewRankName` (vmangos `Guild.h:133`) — the id as text, like every parameter of
                // this packet.
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
        // Cases 0x00–0x0b carry arg1; the sign-on/sign-off pair (0x0c/0x0d) does not, and neither
        // does anything past it — see [`RosterUpdate`]'s census. This is the whole condition.
        if notice.event <= guild_event::UPDATE_ROSTER {
            self.note_roster_update(RosterUpdate::Stale);
        }
    }

    /// `SMSG_GUILD_COMMAND_RESULT` — its effect on the mirror; its line is [`lines`]'.
    ///
    /// The reference's arg-bearing arm is narrow: `0x5e7792` fires the stale signal only for a
    /// successful command `0x13`/`0x14`, or for `result == 0x14 && command == 0x05`. Every other
    /// verdict changes nothing here — the server re-sends the whole roster for anything that did.
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

    /// `SMSG_GUILD_INVITE` — arm the popup's show edge.
    ///
    /// There is no hide edge: `GUILD_INVITE_CANCEL`'s wrapper `0x48f470` has **zero callers
    /// image-wide**, so nothing in 1.12 ever raises it. The FrameXML registers for it and simply
    /// never hears it, which is faithful. We still clear the invitation when it is answered
    /// ([`Self::clear_invite`]) so that a second invitation re-fires the show edge.
    fn apply_invite(&mut self, inviter: String, guild: String) {
        self.pending_invite = Some((inviter, guild));
        self.dirty = true;
    }

    /// Answer the invitation we are holding — the popup's Accept/Decline.
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

    /// `SetGuildRosterShowOffline(flag)`: re-sorts (it is a comparator input) and changes the
    /// count. The reference's worker `0x4d0f70` no-ops entirely when the value is unchanged — no
    /// write, no re-sort, no event — so this does too.
    fn set_show_offline(&mut self, on: bool) {
        if self.show_offline != on {
            self.show_offline = on;
            self.note_roster_update(RosterUpdate::Applied);
        }
    }

    /// `SetGuildRosterSelection(index)`: resolve the 1-based display row to the guid we keep it
    /// as. No event — the reference's own caller repaints itself
    /// (`FriendsFrameGuildStatusButton_OnClick` calls `GuildStatus_Update` on the next line).
    fn select(&mut self, index: u32) {
        let guid = self.guid_at(index).unwrap_or(0);
        if self.selection != guid {
            self.selection = guid;
            self.dirty = true;
        }
    }

    /// The guid at a 1-based display row. The addressable range is the **whole** roster, never the
    /// show-offline count (module doc).
    fn guid_at(&self, index: u32) -> Option<u64> {
        index
            .checked_sub(1)
            .and_then(|i| self.display_order.get(i as usize))
            .copied()
    }

    /// The member at a 1-based display row — the note verbs address a ROW and the wire wants a
    /// NAME, and only the app knows the order that row refers to. Mutable because those same verbs
    /// write the note into the record before deciding whether to send it (see the drain).
    fn member_at_mut(&mut self, index: u32) -> Option<&mut GuildRosterMember> {
        let guid = self.guid_at(index)?;
        self.members.iter_mut().find(|m| m.guid == guid)
    }

    /// How many rows `GetNumGuildMembers()` reports — **an advisory loop bound, not the
    /// addressable range**: the whole roster while show-offline is on, the online tally while it
    /// is off (`0x4d1190` against `0xb73118`/`0xb7311c`). The comparator's pre-gate is what makes
    /// that tally name exactly the array's leading rows ([`sort`]).
    fn num_members(&self) -> usize {
        if self.show_offline {
            self.members.len()
        } else {
            self.members.iter().filter(|m| m.is_online()).count()
        }
    }

    /// Our own rank's rights word — what every `Can*` predicate tests. `0` when no roster has
    /// arrived, which reads as "no permissions" and is the honest answer: we do not know them yet.
    fn own_rights(&self) -> u32 {
        self.rank_rights
            .get(self.rank_index as usize)
            .copied()
            .unwrap_or(0)
    }
}

/// Decompose "days since last logout" the way `GetGuildRosterLastOnline 0x4d14a0` does — a **full**
/// nested decomposition, not just the largest unit.
///
/// VERIFIED at the bytes, constants and all: `years = ftol(d × 1/365)`, `rem = d − years×365`,
/// `months = ftol(rem × 1/30)`, `rem2 = rem − months×30` (narrowed back to an f32 at
/// `0x4d1544 fstp DWORD PTR [ebp-0x10]` and used as that f32 twice), `days = ftol(rem2)`,
/// `hours = ftol((rem2 − days) × 24)` — reads at `0x4d1508`–`0x4d1598`, the five constants read out
/// of the PE at `0x80733c` = 1/365, `0x807338` = 365, `0x807334` = 1/30, `0x807330` = 30,
/// `0x80732c` = 24. `0x40a2b0` is the MSVC `_ftol` (`or ah,0xc` → round toward zero), so every
/// step **truncates**.
///
/// The consumer only ever reads the largest non-zero unit — `GuildFrame_GetLastOnline` tests years,
/// then months, then days, then hours and stops at the first that is neither `0` nor `nil`
/// (`FriendsFrame.lua:957-978`) — so the lower units are invisible to the reference UI and visible
/// to any addon, and filling them is what the engine does.
///
/// An online member carries no float at all on the wire, and `LastOnline::default()` (all zeroes)
/// is exactly the shape that formatter reads as "< an hour", which is also what it shows for nil.
pub(crate) fn last_online(days: f32) -> LastOnline {
    if days.is_nan() || days <= 0.0 {
        // NaN, negative, and an online member's absent field all land here. Truncation toward zero
        // would answer all-zeroes anyway; this is the guard that says so on purpose.
        return LastOnline::default();
    }
    // The x87 path runs in extended precision; f64 is the closest thing that costs nothing. The
    // reciprocals are written the way the reference stores them — f32 roundings of 1/365 and
    // 1/30, multiplied, not divided.
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

/// A unit's guild membership for `GetGuildInfo(unit)` — its own PUBLIC descriptor fields joined
/// against the identity cache, **asking for that identity if we do not hold it**.
///
/// `None` for a guildless unit, for a creature (which has no player block at all), and for a guild
/// whose query has not answered yet: the reference takes the same nil path on the cache-miss leg
/// as on the guildless one (`0x4c93d7 test eax,eax` / `je 0x4c943c`, the branch that pushes
/// nil, nil). A name we do not have yet is not a blank name — and the miss is what *starts* the
/// query, which is the whole of the lazy cache (module doc).
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

/// A unit's guild NAME alone — the **a5 line of the overhead name stack** (`"\n<%s>"`,
/// `0x860f9c`), which the reference resolves through the very same guild-identity cache this
/// serves `GetGuildInfo` from: `0x609085` tests the render mask's bit `0x10`
/// (`UnitNamePlayerGuild`) and then reads `0x5e09f0` off the unit's own `[CGUnit+0xe68]+0x8/0xc`
/// guild GUID.
///
/// [`unit_guild`] without the rank — and, deliberately, **without its two `String` clones**: this
/// is read once per shown player per frame by [`crate::nameplates::drive_nameplates`], whose whole
/// steady-frame design is that a name nothing changed allocates nothing.
///
/// The `None` legs are [`unit_guild`]'s, unchanged: a guildless player, a creature, and a guild id
/// whose `CMSG_GUILD_QUERY` has not answered yet — the last of which is also what *sends* it, so
/// the line appears a round trip later and the rebuild arm picks it up on that frame.
pub(crate) fn unit_guild_name<'a>(
    fields: &ObjectFields,
    guild: &'a mut GuildState,
    commands: &NetCommands,
) -> Option<&'a str> {
    guild
        .resolve_identity(fields.player_guild_id(), commands)
        .map(|identity| identity.name.as_str())
}

/// A unit's guild **tabard**, for the body composite — the emblem five of `SMSG_GUILD_QUERY_RESPONSE`,
/// joined off the unit's own PUBLIC `PLAYER_GUILDID` and asking for the identity if we do not hold
/// it, exactly like [`unit_guild`].
///
/// `None` — no crest painted, so a Guild Tabard keeps its own `Tabard_A_05Default` art — covers
/// **four** cases, and the reference reaches the same nil on all four:
///
/// 1. a guildless wearer — `0x560e30` returns NULL at `0x560e3f` before it even queries;
/// 2. a creature, which has no player block at all;
/// 3. **the query has not answered yet** — the same NULL leg, which is also what *sends* the query.
///    Transient and self-healing: the response bumps [`GuildState::identity_generation`], and the
///    equipment resolver's gate re-runs on that counter, so the tabard re-composites the frame the
///    answer lands. The reference does not even poll for it — `0x5e0650` is a registered arrival
///    callback that re-runs the install;
/// 4. **a guild that has never designed a tabard**, i.e. the `-1` sentinel
///    ([`GuildEmblem::is_designed`]) — the case this must not paint through, because painting it
///    would take cell 4 from the garment and then resolve to no file, leaving a blank tabard.
///
/// All four end in `0x47a610` never being entered, which is exactly why "no crest" and "the
/// garment's own art" are the same outcome: the clear of the three cells lives *inside* the install.
pub(crate) fn unit_guild_emblem(
    fields: &ObjectFields,
    guild: &mut GuildState,
    commands: &NetCommands,
) -> Option<GuildEmblem> {
    guild_emblem(fields.player_guild_id(), guild, commands)
}

/// The same crest for a **corpse**, whose guild id is its own snapshot
/// ([`ObjectFields::corpse_guild`]) rather than the living `PLAYER_GUILDID` — the reference reads
/// `CORPSE_FIELD_GUILD` at `0x5d6edf` and runs the identical name-cache lookup before installing
/// the emblem (`0x5d6ec0`). All four `None` cases
/// above hold unchanged: a guildless owner, a query still in flight, an undesigned crest.
pub(crate) fn corpse_guild_emblem(
    fields: &ObjectFields,
    guild: &mut GuildState,
    commands: &NetCommands,
) -> Option<GuildEmblem> {
    guild_emblem(fields.corpse_guild(), guild, commands)
}

/// The shared body of the two above — one lazy `CMSG_GUILD_QUERY` idiom, one `is_designed` gate.
fn guild_emblem(
    guild_id: u32,
    guild: &mut GuildState,
    commands: &NetCommands,
) -> Option<GuildEmblem> {
    let emblem = guild.resolve_identity(guild_id, commands)?.emblem;
    emblem.is_designed().then_some(emblem)
}

/// The guild family's packet handlers (in the net handler table since 2312),
/// beside the state they drive ([`crate::ui_social::net`]'s shape): the identity cache, the
/// roster, and the `ERR_GUILD_*` lines the engine composes; the guild EVENTS fire off the mirror
/// in [`feed_guild`], on their edges. The ones that owe a line queue the **message id** [`lines`]
/// named, the way `crate::net::apply`'s group shims do — the surface and the sound come off the
/// catalog at the error frame's drain, not from here.
pub(crate) mod net {
    use super::*;
    use crate::ui_action::{UiError, UiErrorKeys};
    use crate::ui_social::SocialState;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

    /// Register the family's handlers — called from [`UiGuildPlugin`]. One per kind, plus the
    /// session-end listener.
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

    /// The sign-on/sign-off pair's trailing guid exists for exactly one purpose — the
    /// four-conjunct display condition on their line — which is why this handler reads the
    /// social lists, the notify knob and our own guid (the condition and its byte
    /// addresses are on [`event`]).
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

    /// The guild session is login-scoped — and strictly, because the next login
    /// may be a *different character*, whose guild id, rank, rights and roster share nothing
    /// with this one's. The identity cache goes too: it is keyed by guild id, so it would
    /// survive correctly, but the reference's own is backed by `guildcache.wdb` and re-primed
    /// lazily, and keeping a cache alive across a socket only to save one query is not worth the
    /// one wrong name a renamed guild would show. A listener on the session end
    /// (a second handler on the kind, after the bridge's own teardown).
    fn on_session_end(In(_): In<SessionEvent>, mut guild: ResMut<GuildState>) {
        *guild = GuildState::default();
    }

    /// Queue what [`lines`] named. The key IS the lookup and the catalog row behind it names the
    /// surface and the sound, so nothing here decides either — which is the difference decision
    /// 2054 made: this used to push composed English straight onto the system chat log, and two
    /// of these messages are not chat lines at all.
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

    /// `SMSG_GUILD_EVENT`. The trailing guid rides only on the sign-on/sign-off pair, and it is
    /// there to answer that pair's **display condition** — which the reference builds out of
    /// **four conjuncts**, all of them in the handler's `0x0c`/`0x0d` arms, each branching to the
    /// same silent exit `0x5e74c9`:
    ///
    /// 1. **there is a local player object.** Ours is "we know our own guid" — the same fact, and
    ///    it is what conjunct 3 needs anyway.
    /// 2. **`guildMemberNotify` is on** (`0x5e733f` / `0x5e73e7`, reading the record's `+0x28`).
    ///    Registered `"0"` — off out of the box, and the reason a stock client is silent here.
    /// 3. **the subject is not you.** The reference compares the wire NAME against the local
    ///    player's (`0x609210` + `SStrCmp`); ours compares the wire GUID against [`SelfGuid`] —
    ///    the same subject by a different key, and the key this arm already carries. It is not
    ///    hypothetical: vmangos's `Guild::BroadcastPacket` walks **every** member including the
    ///    one who just signed on (`Guild.cpp:651-656`), so without this you announce yourself at
    ///    every login.
    /// 4. **the subject is not on your friends list** — `FriendList::FindFriendSlot 0x5ae810`,
    ///    and this is the conjunct benilla had wrong. It read the call as an *ignore* check, so an
    ///    ignored guildmate was silenced (the reference announces them) and a guildmate who is
    ///    also a friend was announced **twice**: `SMSG_FRIEND_STATUS` (`0x5acde6`/`0x5ace08`)
    ///    emits the same two chat ids with no CVar gate at all, and de-duplicating against it is
    ///    this conjunct's entire job.
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

    /// The sign-on/sign-off pair's four-conjunct display condition, as one predicate — see
    /// [`event`] for each conjunct's byte address and why conjunct 4 is a *friends* test.
    ///
    /// Named and separate because it is the part that was wrong, and because a predicate is
    /// testable where a `push_lines` side effect is not.
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
            // No local player (conjunct 1), or a pair carrying no guid at all: the condition is
            // unanswerable, and the reference's silent exit is the honest answer to that.
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

    /// `SMSG_GUILD_INVITE` — the popup's arm edge, plus the notice line the reference prints
    /// beside it.
    pub(crate) fn invite(
        guild: &mut GuildState,
        errors: &mut UiErrorKeys,
        inviter: String,
        guild_name: String,
    ) {
        push_lines(errors, [lines::invite_line(&inviter, &guild_name)]);
        guild.apply_invite(inviter, guild_name);
    }

    /// `SMSG_GUILD_DECLINE` — a line only; there is no state behind it.
    pub(crate) fn decline(errors: &mut UiErrorKeys, name: &str) {
        push_lines(errors, [lines::decline_line(name)]);
    }

    /// `SMSG_GUILD_INFO` — the `/ginfo` answer, two lines and no state.
    ///
    /// Parked rather than composed: its templates are not catalog rows, so the lines are built
    /// where the VM is ([`GuildState::pending_info`]).
    pub(crate) fn info(guild: &mut GuildState, info: GuildInfo) {
        guild.pending_info.push(info);
    }
}

/// **The guild feed, as an orderable thing** — so a system that must run after the guild events
/// have fired can say so without reaching for the function (and dragging its private memo type
/// into the crate's surface). Its one consumer is the chat drain, which the reference orders
/// after the world-enter cascade's events: `ui_chat`'s registration has the addresses.
#[derive(bevy::ecs::schedule::SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct GuildFeed;

/// The guild windows' session: the wire mirror, the VM feed, and the outbound intents.
pub(crate) struct UiGuildPlugin;

/// Guild Member Alert's change callback (1589, 2303) — conjunct 2 of the sign-on/sign-off
/// line's condition; a flag.
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
                    // **Never against the boot VM** (1348/1978, and B376's half of it): the feed
                    // takes the MOTD and the roster edges through a [`crate::ui_script::VmMemo`]
                    // that the entry load does not reset, so an edge spent on a VM with no frames
                    // is spent for the whole login — the "Guild Message of the Day:" line fired
                    // into nothing and never fired again. `ingame_ui_up`'s doc has the window.
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

    /// The sign-on/sign-off line's **four-conjunct** display condition. Every
    /// conjunct gets its own case, because the two that were
    /// wrong were wrong in *opposite* directions and a single happy-path assertion would have
    /// caught neither.
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

        // 2 · the CVar, which ships OFF — so the default client says nothing at all.
        assert!(
            !net::announce_signon(&social, &off, me, Some(5)),
            "guildMemberNotify off silences the whole family"
        );
        assert!(net::announce_signon(&social, &on, me, Some(5)));

        // 3 · not you. vmangos broadcasts the sign-on to EVERY member including the signer
        // (`Guild::BroadcastPacket`, Guild.cpp:651-656), so without this you announce yourself at
        // every login.
        assert!(
            !net::announce_signon(&social, &on, me, Some(1)),
            "your own sign-on is not announced to you"
        );

        // 4 · not a friend — de-duplication against SMSG_FRIEND_STATUS, which says the same thing
        // with no CVar gate of its own. THIS is the conjunct benilla read as an ignore check.
        assert!(
            !net::announce_signon(&social, &on, me, Some(7)),
            "a guildmate who is also a friend is announced by the friend path, not twice"
        );

        // …and the same mislabel's other half: an IGNORED guildmate IS announced. The ignore list
        // has nothing to do with this line.
        assert!(
            social.is_ignored(9),
            "the fixture's ignore really is an ignore"
        );
        assert!(
            net::announce_signon(&social, &on, me, Some(9)),
            "the reference announces an ignored guildmate — 0x5ae810 is not the ignore check"
        );

        // 1 · a local player, and the pair's own guid: either missing leaves the condition
        // unanswerable, and the reference's silent exit is the answer.
        assert!(!net::announce_signon(&social, &on, None, Some(5)));
        assert!(!net::announce_signon(&social, &on, me, None));
    }

    /// `show_offline` changes what `GetNumGuildMembers` counts and how the rows are ordered —
    /// **and never what the roster contains**. The three index-taking bindings address the whole
    /// array, so filtering it would make a stale index name a different member, or none.
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

    /// The four unit boundaries the formatter cascades through, plus the shape below the smallest
    /// one. The largest non-zero unit is what the reference displays; the rest is the remainder
    /// the engine fills in behind it.
    #[test]
    fn last_online_decomposes_largest_unit_first() {
        // Under an hour: every unit zero, which `GuildFrame_GetLastOnline` reads as "< an hour".
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
        // And the full decomposition the reference actually returns rather than only the largest
        // unit: 400.5 days is 1 year, 1 month, 5 days, 12 hours.
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

    /// An empty name in the query response is "no such guild", not a guild called "". It must be
    /// cached (so the query is not re-sent forever) and it must not answer `GetGuildInfo`.
    #[test]
    fn an_empty_query_name_is_a_negative_not_a_blank_guild() {
        let (commands, rx) = net_commands();
        let mut guild = GuildState::default();

        // The miss asks, once, and answers nothing this call — the lazy cache.
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

    /// The one decision that is an infinite loop in one direction and a frozen pane in the other:
    /// a guild EVENT means "re-ask" (arg1), a fresh roster or a local re-order means "repaint"
    /// (no arg1) — and the sign-on/sign-off pair means neither.
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

    /// A guildmate logging in or out is the one guild event that does NOT force a re-request —
    /// cases 0x0c/0x0d are outside the arg-bearing band.
    #[test]
    fn signing_on_and_off_does_not_re_request() {
        let mut guild = GuildState::default();
        for ev in [guild_event::SIGNED_ON, guild_event::SIGNED_OFF] {
            guild.roster_event = None;
            guild.apply_event(&event(ev, &["Tigole"], Some(9)));
            assert_eq!(guild.roster_event, None, "event {ev:#04x}");
        }
        // …while the case immediately below them does.
        guild.apply_event(&event(guild_event::UPDATE_ROSTER, &[], None));
        assert_eq!(guild.roster_event, Some(RosterUpdate::Stale));
    }

    /// The stale edge clears the `GuildRoster()` throttle. Without that, the arg1 the FrameXML
    /// answers with `GuildRoster()` would be swallowed by the 10 s limiter and the "re-request"
    /// would silently do nothing.
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

    /// Only the two arg-bearing command-result arms move the roster; an ordinary refusal changes
    /// nothing here (it says something, and that is [`lines`]' job).
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

    /// Leaving a guild drops the roster of the guild we left — but keeps the identity cache,
    /// which is keyed by id and is still true.
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

    /// **B376, at the line that caused it.** At a login the server sends `SMSG_GUILD_EVENT 0x02`
    /// (the MOTD) a whole packet burst before our own avatar's descriptor carries
    /// `PLAYER_GUILDID`, so the mirror's first sight of our guild id is `0 -> N` — and that is us
    /// being TOLD which guild we are in, not us moving between two. Reading it as a move wiped the
    /// MOTD that had already arrived, in the same `feed_guild` call that would have fired
    /// `GUILD_MOTD` a few lines further down: the login line was destroyed before the edge could
    /// be taken, and the text only came back when the guild pane asked for a roster — the report.
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

    /// Our own rank's rights word is read out of the roster's array by our own descriptor rank,
    /// and answers 0 (no permissions) rather than guessing while no roster has arrived.
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

    /// A roster claiming more ranks than the ten that exist is clamped — the reference's own loop
    /// walks off its array into the member list's control block instead.
    #[test]
    fn an_over_long_rank_array_is_clamped() {
        let mut guild = GuildState::default();
        guild.apply_roster(GuildRoster {
            rank_rights: vec![1; 14],
            ..Default::default()
        });
        assert_eq!(guild.rank_rights.len(), GUILD_RANKS_MAX_COUNT);
    }

    /// A rank rename lands in the identity cache — the reference writes it straight into its own
    /// cached record, so the pushed snapshot is the only thing that can carry it to the pane.
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

    /// The selection is a guid, so it survives a re-sort pointing at the same player — the row
    /// index the VM sees is derived, never stored — and a member who leaves silently becomes
    /// "nothing selected" rather than addressing whoever took their row.
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

        // The list re-orders under it; the same PLAYER stays selected.
        guild.display_order = vec![22, 11];
        assert_eq!(feed::index_of(&guild.display_order, guild.selection), 1);

        guild.select(0);
        assert_eq!(guild.selection, 0, "0 = nothing selected");
        guild.select(9);
        assert_eq!(guild.selection, 0, "past the end selects nothing");

        // Bob leaves. Nothing resets the stored guid — it simply stops resolving.
        guild.selection = 22;
        guild.apply_roster(GuildRoster {
            members: vec![member(11, "Alice", guild_presence::ONLINE)],
            ..Default::default()
        });
        guild.display_order = vec![11];
        assert_eq!(feed::index_of(&guild.display_order, guild.selection), 0);
    }

    /// The invitation is the popup's show edge, and answering it re-arms rather than firing a
    /// cancel — nothing in 1.12 raises `GUILD_INVITE_CANCEL`.
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

    /// `GetGuildInfo(unit)` answers nil for a guildless unit AND for one whose guild query has not
    /// answered — the reference takes the same branch for both (`0x4c93d7`) — and the miss is what
    /// starts the query.
    #[test]
    fn unit_guild_asks_on_a_miss_and_never_names_a_blank_guild() {
        let (commands, rx) = net_commands();
        let mut guild = GuildState::default();
        guild
            .identities
            .insert(7, Some(identity("Legacy", &["GM", "Off"])));

        // A creature, and a guildless player: no PLAYER_GUILDID at all.
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

        // A rank past the identity's ten slots names nothing rather than reading off the end,
        // which is the one place the reference does not bound itself.
        let odd_rank = ObjectFields::from_pairs(&[(191, 7), (192, 99)]);
        assert_eq!(
            unit_guild(&odd_rank, &mut guild, &commands)
                .unwrap()
                .rank_name,
            ""
        );
    }

    /// The overhead a5 line's reader answers exactly what `GetGuildInfo`'s does, on the same
    /// three legs and with the same lazy ask — the one thing that could silently drift between
    /// them is which of the two sends the query, and neither may skip it.
    #[test]
    fn unit_guild_name_is_unit_guilds_name_on_every_leg() {
        let (commands, rx) = net_commands();
        let mut guild = GuildState::default();
        guild
            .identities
            .insert(7, Some(identity("Legacy", &["GM", "Off"])));
        // The negative cache: a query that came back empty is "no such guild", not a blank name.
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
