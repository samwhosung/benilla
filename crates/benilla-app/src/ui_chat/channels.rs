//! The zone-channel auto-join (decision 0288 phase 6's remainder) — the client half of a
//! handshake the server deliberately does not perform.
//!
//! **Nobody joins these for you.** vmangos's `Player::UpdateLocalChannels`
//! (`src/game/Objects/Player.cpp:5121`) is an empty function whose entire body is the comment
//! `// Updated client-side`, so a client that never sends `CMSG_JOIN_CHANNEL` sits in no channel
//! at all — silently, because nothing on the wire complains. That is what benilla did until now:
//! `/join` worked, and General/Trade/LocalDefense simply never existed.
//!
//! The walk is every [`ChatChannels.dbc`](benilla_formats::ChatChannelsCatalog) row whose bit is
//! set in the character's **`ZONECHANNELS` mask** ([`ChannelState::zone_mask`] — seeded from the
//! DBC's `INITIAL` rows on a fresh character, then moved only by confirmed joins and explicit
//! leaves), composed against the player's current **zone** and sent as
//! ordinary joins. It re-runs whenever the zone changes: the zone-dependent rows carry the zone
//! name *inside* the channel name, so crossing a border is genuinely leaving one channel and
//! joining another — LEAVE(old) then JOIN(new), interleaved per slot.
//!
//! The whole mechanism is one function of the reference, `ZoneChannelRefresh 0x49a210`; each part
//! below cites the address it comes from.
//!
//! ## Why this is worth more than three channel names
//!
//! The server answers each join with `SMSG_CHANNEL_NOTIFY`/`YOU_JOINED`, which becomes a
//! `CHAT_MSG_CHANNEL_NOTICE` at the Lua VM (0288 §1's addon-API phase) — and that one event is the
//! condition for **Ace2's initialisation gate**. `AceEvent`'s `activate` registers
//! `CHAT_MSG_CHANNEL_NOTICE` and, on the first one, schedules the function that sets
//! `self.postInit = true` and fires `AceEvent_FullyInitialized`
//! (`AceEvent-2.0.lua:913-947`). Until that happens `AceEvent:IsFullyInitialized()` answers false
//! forever, with no error anywhere — and ~75 FuBar plugins, BigWigs, oRA2, RosterLib, Jostle and
//! AceComm all wait on it. `ui_chat::ace_gate_tests` is that claim, proved against the corpus's
//! own Ace2 chain.

use bevy::prelude::*;

use benilla_assets::LockRecover;
use benilla_formats::ChatChannelsCatalog;

use crate::area::AreaTableRes;
use crate::net::{ClientCommand, NetCommands};

use super::edit::{ChannelState, SlotState};
use super::recruitment::GuildRecruitmentCascade;

/// `AreaTable.dbc` flag `0x08` — vmangos calls it `AREA_FLAG_SLAVE_CAPITAL`, with the comment
/// *"Allow trade channel"* (`src/game/Database/DBCEnums.h:58`). In the shipped 5875 table exactly
/// six rows carry it, all top-level zones and all with flags `0x138`: Undercity, Stormwind City,
/// Ironforge, Orgrimmar, Thunder Bluff and Darnassus. (`0x100` = `AREA_FLAG_CAPITAL` selects the
/// identical six in this build; `0x08` is the bit named for this job, so it is the one read.)
const AREA_FLAG_TRADE_CHANNEL: u32 = 0x08;

/// `AreaTable.dbc` flag `0x200` — the sentinel marking the row whose name is the **shared city
/// word** the Trade channel is named after.
///
/// Not a geographic area at all: the client scans the table once at load for this bit and keeps
/// the row's name at `0xb4e4f0` (single writer `0x4985fd`), then splices it into `Trade - %s`.
/// Exactly one row in the shipped 5875 table carries it — **id 3459, `AreaName[enUS] = "City"`** —
/// which is the whole reason the word appears nowhere in `WoW.exe`. Reading it here rather than
/// hardcoding `"City"` is what keeps a localized install localized.
const AREA_FLAG_CITY_NAME_ROW: u32 = 0x200;

/// The shared city word, scanned out of `AreaTable.dbc` — the client's own load-time scan
/// ([`AREA_FLAG_CITY_NAME_ROW`]). `None` when no row carries the sentinel, in which case the
/// city-named rows are skipped rather than joined under a half-formed name.
pub(crate) fn city_word(areas: &benilla_formats::AreaTableCatalog) -> Option<&str> {
    areas
        .rows()
        .find(|r| r.flags & AREA_FLAG_CITY_NAME_ROW != 0)
        .map(|r| r.name.as_str())
        .filter(|n| !n.is_empty())
}

/// The walk's own state: the zone it last walked, the zone it can act on now, and whether there
/// is a character session to walk for.
///
/// Deliberately **not** a second copy of the joined list. The reference's pass 1 walks the slot
/// array itself (`0x49a284`, stride `0xa0`) and decides leave-vs-join from each slot's own state
/// (`+0x9c`); this walk does the same over [`ChannelState::joined`]. The request
/// side that once lived here (`held`) fell out the moment 2137 made the walk register its slots
/// at send time — from then on the slot array *was* the request side, and a channel joined
/// outside the walk (`/join General`, the guild-recruitment cascade) took a slot the walk did not
/// know about and re-joined it on the next border.
#[derive(Resource, Default)]
pub(crate) struct ZoneChannelWalk {
    /// The top-level `AreaTable` zone the last walk was composed for. `None` = never walked, or
    /// the session ended — either way the next in-world frame re-walks from scratch.
    at: Option<u32>,
    /// **The zone the walk can act on THIS frame** — the reference's `ds:0xb4e314`, the zone id
    /// `UpdateZoneText` writes and both the walk (`0x49a243`) and the guild-recruitment cascade
    /// (`0x49ead7`) read. `None` while any gate holds it (no session, a cinematic, a world still
    /// arriving, no area authority). Published every frame, walked or not, so the cascade one
    /// system over resolves the same zone the walk did rather than deriving its own.
    pub(super) zone_id: Option<u32>,
    /// **Is there a character session to join channels for?** Armed by `EnteredWorldMessage`,
    /// disarmed by [`end_channel_session`] — a positive edge in both directions, and the reason
    /// this is a field rather than an ordering.
    ///
    /// Without it the walk is live on the *logout frame*: `ClientState` has not left `InWorld`
    /// yet, the avatar is still standing there settled, and the session-end clear has just emptied
    /// `at` — so the walk faithfully re-diffs from nothing and sends a JOIN for the zone the
    /// character is *leaving*. Caught live by the `/logout` + `Enter` probe (1284): two joins
    /// stamped at the same millisecond as "tearing down the streamed world".
    live: bool,
}

impl ZoneChannelWalk {
    /// Forget everything: leaving the world took our channel membership with it, so the next entry
    /// must re-join rather than assume. See [`end_channel_session`].
    fn clear_session(&mut self) {
        self.at = None;
        self.zone_id = None;
        self.live = false;
    }
}

/// **Channel membership dies with the character session, so this state must die with it too**
/// (1284).
///
/// Server-side that is not a policy but a destructor: `Player::CleanupChannels`
/// (vmangos `src/game/Objects/Player.cpp:5107`) walks every channel the player is in and leaves
/// it, and it is called from `~Player` and from the logout cleanup. A new character — or the same
/// character after a reconnect — enters the world in **zero** channels, always.
///
/// Ours used to survive that boundary, and the director caught it on a character switch: the walk
/// still held the previous character's `General - Tanaris`, so its first diff on the new
/// character sent LEAVE(Tanaris) — which the new session is genuinely not in, so the server
/// answered "Not on channel 1. General - Tanaris." — and [`ChannelState::joined`] still listed
/// those two dead rows, which is why the real joins came back numbered 3 and 4 instead of 1 and 2.
///
/// Four things end together because they are one fact: the walk's zone, what the server confirmed
/// ([`ChannelState::joined`]), the VM's mirror of the latter (what `GetChannelName` answers an
/// addon), and the guild-recruitment cascade's watcher (its next login must see the guild id as
/// new). The edit box's channel target goes too — it holds the wire name of a channel that no
/// longer exists for us, and a `/2` typed on the new character would otherwise send into it.
///
/// **The `ZONECHANNELS` mask is deliberately not on that list** (2120): it belongs to the
/// character's file, and the login that reads that file is what seats it.
fn end_channel_session(
    script: Option<&mut benilla_ui::script::UiScript>,
    channels: &mut ChannelState,
    walk: &mut ZoneChannelWalk,
    cascade: &mut GuildRecruitmentCascade,
) {
    walk.clear_session();
    cascade.clear_session();
    channels.joined.clear();
    if let Some(script) = script {
        script.set_joined_channels(Vec::new());
    }
}

/// Seed a fresh VM's joined-channel mirror. The mirror is otherwise pushed only
/// on the join/leave edges ([`super::feed`]'s YOU_JOINED / YOU_LEFT arms), and a `/reload`
/// replaces the VM *between* edges — `ChannelState` (server truth) survives, but the new VM's
/// mirror would stay empty: `GetChannelName`/`GetChannelList` answer nothing, `/1`-`/9` routing
/// is dead, and every channel line renders unnumbered until the player happens to join or leave
/// something. A login goes through [`end_session_channels`] + the auto-join walk instead, where
/// this claim pushes the just-cleared (empty) list — a no-op by construction.
pub(super) fn seed_channels(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    channels: Res<ChannelState>,
    mut seeded: Local<crate::ui_script::VmMemo<bool>>,
) {
    let Some(mut script) = script else {
        return;
    };
    if seeded.claim(&script) {
        script.set_joined_channels(channels.names());
    }
}

/// The session-end edge: a confirmed `/logout` back to the glue layer (`OnExit(InWorld)`), which is
/// the character switch the director's screenshot caught.
pub(super) fn end_session_channels(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut channels: ResMut<ChannelState>,
    mut walk: ResMut<ZoneChannelWalk>,
    mut cascade: ResMut<GuildRecruitmentCascade>,
) {
    end_channel_session(
        script.map(NonSendMut::into_inner),
        &mut channels,
        &mut walk,
        &mut cascade,
    );
}

/// The other end: a socket that died. A **recoverable** drop never leaves `InWorld` (0065 keeps the
/// avatar as the local puppet for the reconnect), so the edge above does not fire for it — but the
/// reconnect still builds a fresh `Player` server-side, with the same empty channel list a fresh
/// login has. Both edges therefore clear, and they clear the same things through the same
/// function so neither can drift into being the more thorough one.
pub(super) fn end_session_channels_on_disconnect(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut channels: ResMut<ChannelState>,
    mut walk: ResMut<ZoneChannelWalk>,
    mut cascade: ResMut<GuildRecruitmentCascade>,
    mut disconnects: MessageReader<crate::net::DisconnectedMessage>,
) {
    if disconnects.read().next().is_none() {
        return;
    }
    end_channel_session(
        script.map(NonSendMut::into_inner),
        &mut channels,
        &mut walk,
        &mut cascade,
    );
}

/// Startup: read `ChatChannels.dbc` into [`ChannelState`], which owns it because the two things
/// that need it — composing the auto-join names and answering a chat event's arg7 — are both its
/// business. Absent install ⇒ an empty catalog: no auto-join, arg7 stays 0, nothing errors.
pub(super) fn load_chat_channels(
    mut channels: ResMut<ChannelState>,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_chat_channels_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("chat: {} ChatChannels rows", cat.rows().len());
            channels.channels = cat;
        }
        Err(e) => warn!("chat: ChatChannels.dbc failed to load — no zone channels: {e:#}"),
    }
}

/// The name `row` composes to for a player standing in `zone_name`, or `None` when the
/// composition has nothing to substitute: a zone-dependent row with no zone, or a city-named row
/// with no city word.
///
/// The client has no such guard — its capital arm null-checks the DBC row pointer but not the
/// string, so a locale with the sentinel row's name blank would compose exactly `"Trade - "`
/// (`0x49a2d0`–`0x49a2f6`). We decline instead: joining the wrong
/// channel name is invisible and permanent, and joining none is neither.
fn compose(
    row: &benilla_formats::ChatChannelRow,
    zone_name: &str,
    city_word: Option<&str>,
) -> Option<String> {
    if row.is_zone_dependent() && zone_name.is_empty() {
        return None;
    }
    if row.takes_city_name() && city_word.is_none() {
        return None;
    }
    Some(row.joinable_name(zone_name, city_word.unwrap_or_default()))
}

/// The names the walk registers on a character with **no slots yet** — every `ChatChannels.dbc`
/// row whose bit is set in `zone_mask` and that composes here, in table order. The city gate is
/// deliberately not applied: this is the set that takes *slots*, and
/// [`wanted_channels`] is the subset the gate then lets the walk join. A test lens over
/// [`plan_walk`]'s pass 2 — the walk itself plans over the slot array.
///
/// The reference registers the slot (`0x49b980`) *before* it asks whether the row is eligible:
/// `0x49a50d` precedes the city gate at `0x49a512`. So a character standing outside a capital
/// still holds a `Trade - City` slot — created, never joined, state 3 — and it still occupies
/// **number 2**, because the `N.` a channel line carries is `slot[+0x00]`, the client-local index,
/// never the `ChatChannels.dbc` ChannelID.
#[cfg(test)]
pub(crate) fn tracked_channels(
    catalog: &ChatChannelsCatalog,
    zone_mask: u32,
    zone_name: &str,
    city_word: Option<&str>,
) -> Vec<String> {
    catalog
        .rows()
        .iter()
        .filter(|r| zone_mask & super::edit::zone_bit(r.id) != 0)
        .filter_map(|r| compose(r, zone_name, city_word))
        .collect()
}

/// [`tracked_channels`] with the city gate applied — the names the walk actually **joins** from
/// `zone_name`. `in_city` gates the city-**only** rows (the `0x10` bit); `city_word` is what a
/// city-**named** row (`0x20`) puts in its `%s`.
#[cfg(test)]
pub(crate) fn wanted_channels(
    catalog: &ChatChannelsCatalog,
    zone_mask: u32,
    zone_name: &str,
    in_city: bool,
    city_word: Option<&str>,
) -> Vec<String> {
    catalog
        .rows()
        .iter()
        .filter(|r| zone_mask & super::edit::zone_bit(r.id) != 0)
        .filter(|r| in_city || !r.is_city_only())
        .filter_map(|r| compose(r, zone_name, city_word))
        .collect()
}

/// One step of a walk, in **wire order** — the walk is a plan over the slot array, computed pure
/// and applied in sequence, so that the exact packet-and-state sequence a border produces is a
/// unit-testable value (the harness this module's own tests used to say was
/// missing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WalkStep {
    /// `CMSG_LEAVE_CHANNEL` with the slot's OLD name (`0x49a367`) — only from a server-confirmed
    /// slot (`0x49a35e`).
    Leave(String),
    /// `CMSG_JOIN_CHANNEL` (`0x49a3e1` in pass 1, `0x49a56e` in pass 2).
    Join(String),
    /// `0x49bc50` — the slot renamed **in place, at send time**, before the server can answer the
    /// LEAVE that just went out ([`ChannelState::rename_slot`] carries the whole why).
    Rename { old: String, new: String },
    /// `0x49b980` — a slot claimed for a row not yet tracked, **before** the gate (2137).
    Register(String),
    /// `0x49bcf0` — state 3: the row stopped applying; the record and its number survive.
    Suspend(String),
    /// The state-3 bypass's own write, with the name unchanged: `0x49bc50` writes
    /// `(old == 3) ? 1 : 2`, so a suspended slot re-joining under its own name goes to **1** —
    /// a plain `YOU_JOINED` when it confirms, never `YOU_CHANGED`.
    Confirm(String),
}

/// **The walk, as a plan** — `ZoneChannelRefresh 0x49a210`'s two passes over the current slot
/// array and the mask, for a player standing in `zone_name`.
///
/// **Pass 1 — the slots already held**, in slot order (`0x49a284`), each one whose name resolves
/// to a `ChatChannels.dbc` row. The row's name is composed for this zone, **whatever the mask
/// says** — the reference's pass 1 reads no mask bit, and a slot exists only because a join
/// confirmed it or a walk registered it. Then, per slot:
///
/// - **name unchanged, slot suspended** ⇒ the state-3 bypass (`0x49a31c`): eligible here ⇒
///   `Confirm` + `Join` — walking back into a city re-joins `Trade - City` under a name that never
///   changed; not eligible ⇒ nothing.
/// - **name unchanged, otherwise** ⇒ nothing, unless the row stopped being eligible: then
///   `Leave` (only if server-confirmed) and `Suspend` — walking out of the city; the LEAVE goes
///   out *before* the gate (`0x49a3b8`) suspends the slot because it sits earlier in the same
///   iteration.
/// - **name moved** ⇒ `Leave(old)` if server-confirmed (`0x49a35e`), then the gate: eligible ⇒
///   `Rename` + `Join(new)`; not eligible ⇒ `Suspend(old)`.
/// - **the row no longer composes here** (no city word for a city-named row) ⇒ treated as not
///   eligible: `Leave` if confirmed, `Suspend`. The reference would compose a half-formed name
///   instead ([`compose`]).
///
/// **Pass 2 — rows with no slot** (`0x49a468`), in DBC order, **only those whose mask bit is set**
/// (`0x49a494`, the live predicate). Each composes, takes a slot (`Register`,
/// before the gate), and then `Join`s or is `Suspend`ed. This is the one place the mask decides:
/// a `/leave General` clears bit 0 and frees the slot, and from then on — this session and every
/// later one, because the mask is the character's file — General is never registered again until
/// a `/join General` confirms and sets the bit back.
///
/// LEAVE strictly precedes JOIN per slot, interleaved across slots — never a batch of leaves then
/// a batch of joins. The retail 1.8.1 Winterspring sniff shows exactly this shape.
pub(crate) fn plan_walk(
    channels: &ChannelState,
    zone_name: &str,
    in_city: bool,
    city_word: Option<&str>,
) -> Vec<WalkStep> {
    use WalkStep as W;
    let catalog = &channels.channels;
    let eligible = |row: &benilla_formats::ChatChannelRow| in_city || !row.is_city_only();
    let mut steps = Vec::new();
    let mut tracked_rows: Vec<u32> = Vec::new();

    // Pass 1.
    for slot in channels.joined.iter().flatten() {
        let Some(row) = catalog.row_for_name(&slot.name) else {
            continue; // a custom channel: not the walk's
        };
        tracked_rows.push(row.id);
        let old = slot.name.as_str();
        match compose(row, zone_name, city_word) {
            Some(new) if new.eq_ignore_ascii_case(old) => {
                if slot.state == SlotState::Suspended {
                    if eligible(row) {
                        steps.push(W::Confirm(old.to_string()));
                        steps.push(W::Join(old.to_string()));
                    }
                } else if !eligible(row) {
                    if slot.state == SlotState::Joined {
                        steps.push(W::Leave(old.to_string()));
                    }
                    steps.push(W::Suspend(old.to_string()));
                }
            }
            Some(new) => {
                if slot.state == SlotState::Joined {
                    steps.push(W::Leave(old.to_string()));
                }
                if eligible(row) {
                    steps.push(W::Rename {
                        old: old.to_string(),
                        new: new.clone(),
                    });
                    steps.push(W::Join(new));
                } else {
                    steps.push(W::Suspend(old.to_string()));
                }
            }
            None => {
                if slot.state == SlotState::Joined {
                    steps.push(W::Leave(old.to_string()));
                }
                if slot.state != SlotState::Suspended {
                    steps.push(W::Suspend(old.to_string()));
                }
            }
        }
    }

    // Pass 2.
    for row in catalog.rows() {
        if !channels.zone_row_wanted(row.id) || tracked_rows.contains(&row.id) {
            continue;
        }
        let Some(name) = compose(row, zone_name, city_word) else {
            continue;
        };
        steps.push(W::Register(name.clone()));
        if eligible(row) {
            steps.push(W::Join(name));
        } else {
            steps.push(W::Suspend(name));
        }
    }
    steps
}

/// Apply a plan in order: packets to the wire, slot writes to [`ChannelState`]. Answers whether
/// the slot *names* moved (a rename or a registration), which is the VM mirror's cue.
fn apply_walk(steps: &[WalkStep], channels: &mut ChannelState, commands: &NetCommands) -> bool {
    let mut names_moved = false;
    for step in steps {
        match step {
            WalkStep::Leave(name) => {
                debug!("chat: leaving zone channel {name:?}");
                let _ = commands
                    .0
                    .send(ClientCommand::LeaveChannel { name: name.clone() });
            }
            WalkStep::Join(name) => {
                debug!("chat: joining zone channel {name:?}");
                let _ = commands.0.send(ClientCommand::JoinChannel {
                    name: name.clone(),
                    password: String::new(),
                });
            }
            WalkStep::Rename { old, new } => {
                if let Some(n) = channels.rename_slot(old, new) {
                    debug!("chat: zone channel slot {n} renamed {old:?} → {new:?}");
                    names_moved = true;
                }
            }
            WalkStep::Register(name) => match channels.claim_slot(name) {
                Some(n) => {
                    debug!("chat: zone channel {name:?} registered as slot {n}");
                    names_moved = true;
                }
                None => warn!(
                    "chat: no free slot for zone channel {name:?} — all {} are taken",
                    super::edit::MAX_CHANNELS
                ),
            },
            WalkStep::Suspend(name) => {
                if let Some(n) = channels.suspend_slot(name) {
                    debug!("chat: zone channel slot {n} ({name:?}) suspended — not eligible here");
                }
            }
            WalkStep::Confirm(name) => channels.confirm_slot(name),
        }
    }
    names_moved
}

/// Is the zone under the player the one they are actually standing in?
///
/// The walk's gate (1280). Three states answer no, and each one cost real wire traffic before this
/// existed: no avatar at all, an avatar not yet active, and — the expensive one — an avatar still
/// **settling**, which is benilla's stand-in for the reference's loading screen: the body has been
/// snapped but the destination's terrain and WMOs are still arriving, so the leaf area under it is
/// still moving. The reference walks *after* that window, always; we polled inside it.
fn zone_is_settled(player: Option<&crate::player::Player>) -> bool {
    player.is_some_and(|p| p.active && !p.settling)
}

/// The walk: re-join the zone channels whenever the zone changes, and drop the ones that no
/// longer apply.
///
/// Reads the zone the same way [`crate::area`]'s splash does — the leaf `AreaTable` id under the
/// player, walked to its top-level parent — but takes the **real** zone name rather than the
/// display text. That is not a preference: the client composes from `GetRealZoneText`'s own cache
/// (`0xb4b404`, written from the parent zone's `AreaName_lang`), and the indoor/WMO name override
/// `0x67e670` rewrites only the slot feeding `GetZoneText`, never the one that becomes a channel
/// name. So a building never renames your channel.
///
/// **Timing:** the client re-walks inside `UpdateZoneText 0x494780` on every zone change (callsite
/// `0x494931`), immediately *before* it fires `ZONE_CHANGED_NEW_AREA`, plus once at world
/// entry. This polls the same two inputs each frame and early-outs when they have not moved, which
/// reaches the same states; it is a poll rather than a hook because the zone is already derived
/// here, not published as an event payload.
///
/// **…but NOT while the world is still arriving** ([`Player::settling`], 1280). The reference's
/// "world entry" is behind a loading screen: by the time it walks, the destination ADT and its WMOs
/// are resident and the zone under the player is final. Ours streams asynchronously, so the leaf
/// area moves *twice* on the way in — first while the body still holds a pre-snap position (the
/// map centre: tile 32,32 is Eastern Plaguelands on map 0, The Barrens on map 1 — the two bogus
/// zones in the director's login screenshot), then again when the WMO interior claim lands a beat
/// after the outdoor MCNK area (Tanaris → Caverns of Time, reproduced live). Each transient cost a
/// real JOIN and a real LEAVE on the wire, for zones the player was never in — chat lines and all.
/// `settling` is exactly the loading screen's own gate (released by the terrain streamer once the
/// destination is resident), which makes this the reference's timing rather than an
/// arbitrary debounce.
///
/// **…and NOT before the chat cache has seated the mask** ([`ChannelState::zone_mask`] is `None`
/// until then). The reference's walk bails on its "chat system ready" flag (`0x49a219`), which the
/// cache loader sets as its last act before calling the walk itself (`0x499a18`/`0x499a22`). The
/// mask is the walk's row set, so a walk before the seat would join nothing and
/// nothing would call it again.
///
/// **Selection is the `ZONECHANNELS` mask** — the reference's live predicate `0x49a494`, never
/// `flags & INITIAL` read fresh. The DBC bit only seeds the mask on a character with no file
/// (`0x4997fc`). This is what makes `/leave General` stick: the leave clears the bit in the
/// character's file, and the next login's walk never registers the row ([`plan_walk`]).
pub(super) fn auto_join_zone_channels(
    commands: Res<NetCommands>,
    mut channels: ResMut<ChannelState>,
    areas: Option<Res<AreaTableRes>>,
    world: benilla_world::world_point::WorldPoint,
    // One param (clippy's argument ceiling), and they belong together: both answer "is the zone
    // under the player the one this walk may act on?" — the body's own settle, and whether the
    // view has flown off it for a cinematic.
    body: (
        Option<Res<crate::player::Player>>,
        Option<Res<crate::cinematic::Cinematic>>,
    ),
    mut walk: ResMut<ZoneChannelWalk>,
    mut entered: MessageReader<crate::net::EnteredWorldMessage>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut catalog_fed: Local<crate::ui_script::VmMemo<Option<(String, bool)>>>,
) {
    let (player, cinematic) = (&body.0, &body.1);
    // World entry arms the walk; the session-end clears disarm it ([`end_session_channels`] and its
    // disconnect twin, which own the whole fact — walk, confirmed list, VM mirror, edit target).
    if entered.read().next().is_some() {
        walk.live = true;
    }

    // **A cinematic suppresses the rejoin, and it resumes when the shot ends.** Two of the ten
    // sites that read the reference's cinematic-state cell exist for exactly this — `0x49491e`
    // (the zone-text update) and `0x5ff566` (a `UPDATEFLAGS` reflex) both skip
    // `ZoneChannelRefresh` (`0x49a210`) while one runs, and `EndCinematic` calls it once at
    // `0x48f1d0` (the cell is `[0xb4e310]`; the ten sites are its complete census).
    //
    // The walk stays armed and only its *zone* goes unknown, so "rejoin once at the end" is what
    // the first frame after the shot already does — there is nothing to re-arm. It matters because
    // a race intro flies the streaming focus hundreds of yards off the body, so `world.area()`
    // changes under a player who has not moved, and the joins would otherwise fire against zones
    // they are only *looking* at.
    let cinematic_running = cinematic.as_deref().is_some_and(|c| c.is_playing());
    let areas = areas.as_deref();

    // **The zone the walk may act on** — `None` whenever any gate says the leaf under the player
    // is not the one they are standing in: no character session yet ([`ZoneChannelWalk::live`]), a
    // cinematic flying the streaming focus off the body, a world still arriving ([`zone_is_settled`]),
    // or an area authority with no answer for this session (decision 2130 made that a real state
    // again rather than the previous character's zone). Published for the cascade beside it.
    let zone_id = (walk.live && !cinematic_running && zone_is_settled(player.as_deref()))
        .then(|| {
            let areas = areas?;
            world.area().and_then(|leaf| areas.0.top_zone(leaf))
        })
        .flatten();
    walk.zone_id = zone_id;
    let zone = zone_id
        .and_then(|id| areas?.0.get(id))
        .map(|row| (row.name.clone(), row.flags & AREA_FLAG_TRADE_CHANNEL != 0));

    // **The VM's zone-channel catalog is a `ChatChannels.dbc` fact, and it is fed BEFORE any of
    // those gates** — because the thing the verbs read out of it is the row **Shortcut**, which no
    // zone ever changes.
    //
    // It used to be fed only once a zone was known, and an empty catalog is not "no zone yet" to
    // any of its three readers — it is *"no such built-in channel"*. `AddChatWindowChannel(1,
    // "General")` took the no-match leg and stored `("General", 0)` as a **custom** channel, which
    // the chat cache then wrote out as a name in the window's `CHANNELS` block with its zone bit
    // dropped from `ZONECHANNELS`; the director's own `Onewarrior` file carries exactly that
    // damage, and a window that has lost a zone channel's id drops every line the stock
    // `ChatFrame_OnEvent` matches by id (ref `ChatFrame.lua` l.1379). `JoinChannelByName("General")`
    // was worse: `(0, nil)` is the custom-channel leg, so it **sent `CMSG_JOIN_CHANNEL("General")`**
    // — a real custom channel of that name on the server.
    //
    // Fed zone-less, every zone-dependent row carries `resolved: None`, which is the reference's
    // own "matched a row but there is no zone text yet" leg: `AddChatWindowChannel` stores nothing
    // and answers nil (`0x4a10d9`), `JoinChannelByName` returns nil and sends nothing
    // (`0x49ece5`). Right answer instead of a wrong one, and the walk re-feeds the
    // moment a zone lands.
    let mut script = script;
    if !channels.channels.is_empty() {
        if let Some(script) = script.as_mut() {
            let at = zone.clone().unwrap_or_default();
            let fed = catalog_fed.get(script);
            if fed.as_ref() != Some(&at) {
                script.set_zone_channel_catalog(zone_channel_catalog(
                    &channels.channels,
                    &at.0,
                    at.1,
                    areas.and_then(|a| city_word(&a.0)),
                ));
                *fed = Some(at);
            }
        }
    }

    let (Some(zone_id), Some((zone_name, in_city)), Some(areas)) = (zone_id, zone, areas) else {
        return; // nothing to walk against yet
    };
    if channels.zone_mask.is_none() {
        return; // the chat cache has not seated the mask — the reference's ready-flag bail
    }
    if walk.at == Some(zone_id) {
        return; // same zone as last frame — the common case, and free
    }

    let steps = plan_walk(&channels, &zone_name, in_city, city_word(&areas.0));
    if apply_walk(&steps, &mut channels, &commands) {
        if let Some(script) = script.as_mut() {
            // `GetChannelName(n)` answers the new name from this frame on, as it does in the
            // reference — the slot moved, not the numbering.
            script.set_joined_channels(channels.names());
        }
    }
    walk.at = Some(zone_id);
}

/// **The zone-channel catalog goes into the VM before a single interface file runs** (decision
/// 2241, through the seam 2240 established).
///
/// The walk above already feeds it "before any of those gates" — but its gates are the *zone's*,
/// and the load edge is earlier than all of them: since 2226 the entry mints a fresh VM and runs
/// FrameXML, every addon's file scope, `ADDON_LOADED`, `VARIABLES_LOADED` and `PLAYER_LOGIN` inside
/// one exclusive call, and this system's first `Update` tick is after that whole burst. An empty
/// catalog is not "no zone yet" to the three verbs that read it — it is *"no such built-in
/// channel"*, the leg whose damage is documented at length above: a `General` filed as a **custom**
/// channel in the chat cache, and a real `CMSG_JOIN_CHANNEL("General")` on the wire.
///
/// Zone-less on purpose: at this edge the zone is unknowable by construction (the walk is not armed
/// — `EnteredWorldMessage` is read in `Update` — and the body is still settling), and zone-less is
/// the right content rather than a degraded one, for the reason [`auto_join_zone_channels`] gives.
/// The walk re-feeds with the resolved names the moment a zone lands, and pays one redundant
/// identical push per login for it: its memo is a `VmMemo` on its own `Local`, which this call
/// cannot reach, and a resource it could reach would not survive a `/reload` correctly.
pub(crate) fn seed_zone_channel_catalog(
    world: &mut World,
    script: &mut benilla_ui::script::UiScript,
) {
    let Some(channels) = world.get_resource::<ChannelState>() else {
        return;
    };
    if channels.channels.is_empty() {
        return; // no `ChatChannels.dbc` — the no-install case, a no-op exactly as in the walk
    }
    let city = world
        .get_resource::<AreaTableRes>()
        .and_then(|a| city_word(&a.0));
    script.set_zone_channel_catalog(zone_channel_catalog(&channels.channels, "", false, city));
}

/// Every `ChatChannels.dbc` row as the VM needs it: the id, the Shortcut the verbs
/// compare a typed name against (`0x4a10b2`), the name composed for `zone_name` — `None` when the
/// composition has nothing to substitute (a zone-dependent row with no zone, a city row with no
/// city word), which is the verbs' nil leg — and whether `EnumerateServerChannels 0x4a1790` lists
/// it here (a city-only row, `flags & 0x10`, only in a city).
pub(crate) fn zone_channel_catalog(
    catalog: &ChatChannelsCatalog,
    zone_name: &str,
    in_city: bool,
    city_word: Option<&str>,
) -> Vec<benilla_ui::script::ZoneChannelRow> {
    catalog
        .rows()
        .iter()
        .map(|r| benilla_ui::script::ZoneChannelRow {
            id: r.id,
            shortcut: r.shortcut.clone(),
            resolved: compose(r, zone_name, city_word),
            listed: !r.is_city_only() || in_city,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::ChatChannelRow;
    use WalkStep as W;

    /// The shipped table, hand-built so this runs without an install (the real rows are asserted
    /// against the DBC in `benilla_formats::chat_channels`).
    fn catalog() -> ChatChannelsCatalog {
        ChatChannelsCatalog::from_rows(
            [
                (1, 0x00003, "General - %s", "General"),
                (2, 0x0003B, "Trade - %s", "Trade"),
                (22, 0x10003, "LocalDefense - %s", "LocalDefense"),
                (23, 0x10004, "WorldDefense", "WorldDefense"),
                (24, 0x00000, "LookingForGroup", "LookingForGroup"),
                (25, 0x20032, "GuildRecruitment - %s", "GuildRecruitment"),
            ]
            .into_iter()
            .map(|(id, flags, pattern, shortcut)| ChatChannelRow {
                id,
                flags,
                pattern: pattern.into(),
                shortcut: shortcut.into(),
            })
            .collect(),
        )
    }

    /// The city word as the shipped `AreaTable.dbc` supplies it (row 3459, the `Flags & 0x200`
    /// sentinel — see [`city_word`]).
    const CITY: Option<&str> = Some("City");

    /// A fresh character's mask: the DBC's three `INITIAL` rows, `0x200003` (seeded at `0x4997fc`).
    const SEED: u32 = 0x0020_0003;
    /// `1 << (25 - 1)` — `GuildRecruitment`'s bit, which the cascade's confirmed join adds.
    const GUILD_RECRUITMENT_BIT: u32 = 1 << 24;

    /// A [`ChannelState`] over the shipped table with the mask seated at `mask`.
    fn state(mask: u32) -> ChannelState {
        ChannelState {
            channels: catalog(),
            zone_mask: Some(mask),
            ..Default::default()
        }
    }

    fn joined(name: &str) -> Option<crate::ui_chat::edit::ChannelSlot> {
        Some(crate::ui_chat::edit::ChannelSlot::joined(name))
    }

    fn suspended(name: &str) -> Option<crate::ui_chat::edit::ChannelSlot> {
        Some(crate::ui_chat::edit::ChannelSlot {
            name: name.into(),
            state: SlotState::Suspended,
        })
    }

    fn s(v: &str) -> String {
        v.to_string()
    }

    /// Out in the world: General and LocalDefense, both zone-named. Trade is a city channel and
    /// stays out; WorldDefense, LookingForGroup and GuildRecruitment have no bit in a fresh mask.
    #[test]
    fn an_ordinary_zone_joins_general_and_local_defense() {
        assert_eq!(
            wanted_channels(&catalog(), SEED, "Elwynn Forest", false, CITY),
            vec!["General - Elwynn Forest", "LocalDefense - Elwynn Forest"]
        );
    }

    /// Inside a capital, Trade joins too — under the one shared name, not the city's own.
    #[test]
    fn a_capital_adds_the_shared_trade_channel() {
        assert_eq!(
            wanted_channels(&catalog(), SEED, "Stormwind City", true, CITY),
            vec![
                "General - Stormwind City",
                "Trade - City",
                "LocalDefense - Stormwind City",
            ]
        );
    }

    /// No zone name ⇒ no zone-dependent join. "General - " is a real channel on the server, and
    /// joining it would be a bug nothing else would ever report.
    #[test]
    fn an_unknown_zone_joins_nothing_zone_dependent() {
        assert!(wanted_channels(&catalog(), SEED, "", false, CITY).is_empty());
        assert!(wanted_channels(&catalog(), SEED, "", true, CITY).is_empty());
    }

    /// **The walk does not believe the zone until the world has settled** (1280).
    ///
    /// The director's login screenshot is what this guards: three zones' channels joined on the
    /// way in — two of them the map centre (tile 32,32 = Eastern Plaguelands on map 0, The Barrens
    /// on map 1), read while the body still sat at its pre-snap position — and then leave requests
    /// for zones they had never been in, which the server answered "Not on channel …". Every one
    /// of those lines existed because the walk ran a frame too early.
    #[test]
    fn the_walk_waits_for_the_world_under_the_player() {
        use crate::player::Player;

        assert!(!zone_is_settled(None), "no avatar: no zone to believe");

        let mut p = Player::default();
        assert!(!zone_is_settled(Some(&p)), "inactive avatar");

        p.active = true;
        p.settling = true;
        assert!(
            !zone_is_settled(Some(&p)),
            "settling — the destination's terrain and WMOs are still arriving, so the leaf area \
             under the body is still moving"
        );

        p.settling = false;
        assert!(zone_is_settled(Some(&p)), "settled: the zone is now final");
    }

    /// **Leaving the world ends the channel session — every half of it but the mask** (1284,
    /// 2120).
    ///
    /// The director's character switch: the walk still held the previous character's
    /// `General - Tanaris`, so its first diff on the new character sent a LEAVE the new session
    /// answered "Not on channel 1. General - Tanaris.", and the stale confirmed list pushed the
    /// real joins to slots 3 and 4. Server-side there is nothing to keep — `Player::CleanupChannels`
    /// runs in `~Player` — so every one of these must be empty at the next world entry. The mask
    /// is the one survivor: it is the character's file, not the session's.
    #[test]
    fn leaving_the_world_ends_the_channel_session() {
        let mut channels = state(SEED);
        channels.joined = vec![
            joined("General - Tanaris"),
            joined("LocalDefense - Tanaris"),
        ];
        let mut walk = ZoneChannelWalk {
            at: Some(440),
            zone_id: Some(440),
            live: true,
        };
        let mut cascade = GuildRecruitmentCascade::default();
        cascade.observe_guild_id(7);

        // No VM in a unit test; the mirror leg is the one line this cannot reach, and
        // `end_session_channels` is a thin wrapper over exactly this call.
        end_channel_session(None, &mut channels, &mut walk, &mut cascade);

        assert_eq!(walk.at, None, "the next entry re-walks from scratch");
        assert_eq!(walk.zone_id, None);
        assert!(!walk.live);
        assert!(
            channels.joined.is_empty(),
            "the confirmed list is the server's, and the server just destroyed it — a survivor \
             here is what renumbers the next character's channels"
        );
        assert_eq!(
            channels.zone_mask,
            Some(SEED),
            "the mask is the character's file, not the session's (2120)"
        );
        assert!(
            cascade.observe_guild_id(7),
            "the cascade's watcher forgot the guild id, so the next login's first sight of it is \
             a change again — the reference's player-create trigger"
        );
    }

    /// No city word (a locale whose sentinel row ships blank) ⇒ the city-named rows are skipped,
    /// not composed as `"Trade - "`. The client has no such guard; declining is ours.
    #[test]
    fn a_missing_city_word_skips_the_trade_channel_rather_than_half_naming_it() {
        assert_eq!(
            wanted_channels(&catalog(), SEED, "Stormwind City", true, None),
            vec!["General - Stormwind City", "LocalDefense - Stormwind City"]
        );
    }

    /// A zone change **renames** a row rather than adding one: both zones' General lines carry
    /// ChannelID 1, which is what makes the walk's diff a leave/join pair on one row instead of an
    /// unrelated drop and add.
    #[test]
    fn crossing_a_zone_border_renames_the_same_rows() {
        let cat = catalog();
        let felwood = wanted_channels(&cat, SEED, "Felwood", false, CITY);
        let winterspring = wanted_channels(&cat, SEED, "Winterspring", false, CITY);
        assert_ne!(felwood, winterspring, "the names differ");
        let ids = |v: &[String]| -> Vec<u32> { v.iter().map(|n| cat.zone_channel_id(n)).collect() };
        assert_eq!(
            ids(&felwood),
            ids(&winterspring),
            "…but they are the same DBC rows: [1, 22] either side of the border"
        );
        assert_eq!(ids(&felwood), vec![1, 22]);
    }

    /// **A fresh character's first walk, out in the world** — pass 2 over the mask, in DBC order:
    /// every composable mask row takes a slot *before* the gate decides join-versus-suspend,
    /// which is what gives `Trade - City` number 2 on a character nowhere near a
    /// city. `/2` therefore reaches Trade here, as it does in the reference.
    #[test]
    fn a_first_walk_registers_every_mask_row_before_the_gate() {
        let mut c = state(SEED);
        let steps = plan_walk(&c, "Elwynn Forest", false, CITY);
        assert_eq!(
            steps,
            vec![
                W::Register(s("General - Elwynn Forest")),
                W::Join(s("General - Elwynn Forest")),
                W::Register(s("Trade - City")),
                W::Suspend(s("Trade - City")),
                W::Register(s("LocalDefense - Elwynn Forest")),
                W::Join(s("LocalDefense - Elwynn Forest")),
            ]
        );
        // Applied, the numbering is 1 General, 2 Trade, 3 LocalDefense.
        apply_steps(&steps, &mut c);
        assert_eq!(c.number_of("General - Elwynn Forest"), Some(1));
        assert_eq!(c.number_of("Trade - City"), Some(2));
        assert_eq!(c.slot_state("Trade - City"), Some(SlotState::Suspended));
        assert_eq!(c.number_of("LocalDefense - Elwynn Forest"), Some(3));

        // Inside a capital every row joins, and the numbering agrees — the case that hid 2137.
        let steps = plan_walk(&state(SEED), "Stormwind City", true, CITY);
        assert_eq!(
            steps,
            vec![
                W::Register(s("General - Stormwind City")),
                W::Join(s("General - Stormwind City")),
                W::Register(s("Trade - City")),
                W::Join(s("Trade - City")),
                W::Register(s("LocalDefense - Stormwind City")),
                W::Join(s("LocalDefense - Stormwind City")),
            ]
        );
    }

    /// Apply a plan the way the system does, minus the wire — the slot writes only.
    fn apply_steps(steps: &[WalkStep], c: &mut ChannelState) {
        for step in steps {
            match step {
                W::Leave(_) | W::Join(_) => {}
                W::Rename { old, new } => {
                    c.rename_slot(old, new);
                }
                W::Register(name) => {
                    c.claim_slot(name);
                }
                W::Suspend(name) => {
                    c.suspend_slot(name);
                }
                W::Confirm(name) => c.confirm_slot(name),
            }
        }
    }

    /// **A border crossing is LEAVE(old) → rename → JOIN(new), per slot, interleaved, in slot
    /// order** — pass 1 over the slot array (`0x49a284`; the retail 1.8.1 Winterspring sniff is the
    /// control). The suspended Trade slot in the middle is walked too and does nothing: its
    /// name never moves and it is not eligible here.
    #[test]
    fn a_border_crossing_is_leave_rename_join_per_slot_in_slot_order() {
        let mut c = state(SEED);
        c.joined = vec![
            joined("General - Felwood"),
            suspended("Trade - City"),
            joined("LocalDefense - Felwood"),
        ];
        let steps = plan_walk(&c, "Winterspring", false, CITY);
        assert_eq!(
            steps,
            vec![
                W::Leave(s("General - Felwood")),
                W::Rename {
                    old: s("General - Felwood"),
                    new: s("General - Winterspring")
                },
                W::Join(s("General - Winterspring")),
                W::Leave(s("LocalDefense - Felwood")),
                W::Rename {
                    old: s("LocalDefense - Felwood"),
                    new: s("LocalDefense - Winterspring")
                },
                W::Join(s("LocalDefense - Winterspring")),
            ]
        );
        apply_steps(&steps, &mut c);
        assert_eq!(
            c.number_of("General - Winterspring"),
            Some(1),
            "renamed in place"
        );
        assert_eq!(
            c.slot_state("General - Winterspring"),
            Some(SlotState::Renamed)
        );
        assert_eq!(c.number_of("Trade - City"), Some(2), "untouched");
        assert_eq!(c.number_of("LocalDefense - Winterspring"), Some(3));
    }

    /// **The slot number never moves across the whole city round trip**, and the
    /// walk drives exactly the reference's state sequence for `Trade - City`: joined on the way
    /// in, LEAVE-then-suspend on the way out (`0x49bcf0`, record kept), and back to *joined at
    /// send time* on the way back in — the state-3 bypass, where the reference's rename writes
    /// `(old == 3) ? 1 : 2` and so lands on **1**: a plain `YOU_JOINED`, never `Changed Channel`.
    #[test]
    fn a_city_round_trip_keeps_the_slot_and_its_number() {
        let mut c = state(SEED);
        c.joined = vec![
            joined("General - Stormwind City"),
            joined("Trade - City"),
            joined("LocalDefense - Stormwind City"),
        ];

        // Out of the city.
        let steps = plan_walk(&c, "Elwynn Forest", false, CITY);
        assert_eq!(
            steps,
            vec![
                W::Leave(s("General - Stormwind City")),
                W::Rename {
                    old: s("General - Stormwind City"),
                    new: s("General - Elwynn Forest")
                },
                W::Join(s("General - Elwynn Forest")),
                W::Leave(s("Trade - City")),
                W::Suspend(s("Trade - City")),
                W::Leave(s("LocalDefense - Stormwind City")),
                W::Rename {
                    old: s("LocalDefense - Stormwind City"),
                    new: s("LocalDefense - Elwynn Forest")
                },
                W::Join(s("LocalDefense - Elwynn Forest")),
            ]
        );
        apply_steps(&steps, &mut c);
        assert_eq!(c.slot_state("Trade - City"), Some(SlotState::Suspended));
        assert_eq!(c.number_of("Trade - City"), Some(2));
        assert_eq!(
            c.number_of("LocalDefense - Elwynn Forest"),
            Some(3),
            "and nothing above it renumbered — the slot went quiet, it did not go away"
        );
        // The notices confirm the two renamed rows.
        c.confirm_slot("General - Elwynn Forest");
        c.confirm_slot("LocalDefense - Elwynn Forest");

        // Back in: the bypass sends the join AND clears the suspension in the same pass, with no
        // LEAVE — the player is not on the channel.
        let steps = plan_walk(&c, "Stormwind City", true, CITY);
        assert_eq!(
            steps,
            vec![
                W::Leave(s("General - Elwynn Forest")),
                W::Rename {
                    old: s("General - Elwynn Forest"),
                    new: s("General - Stormwind City")
                },
                W::Join(s("General - Stormwind City")),
                W::Confirm(s("Trade - City")),
                W::Join(s("Trade - City")),
                W::Leave(s("LocalDefense - Elwynn Forest")),
                W::Rename {
                    old: s("LocalDefense - Elwynn Forest"),
                    new: s("LocalDefense - Stormwind City")
                },
                W::Join(s("LocalDefense - Stormwind City")),
            ]
        );
        apply_steps(&steps, &mut c);
        assert_eq!(c.slot_state("Trade - City"), Some(SlotState::Joined));
        assert_eq!(c.number_of("Trade - City"), Some(2), "still slot 2");
    }

    /// **`/leave General` stays left — this session and the next** (the director's
    /// report: *"/leave General does not survive a relog"*).
    ///
    /// The explicit leave clears the row's bit (`0x49f10a`) and the server's `YOU_LEFT` frees the
    /// slot (`0x49bbd0`). From then on pass 1 has no General slot to walk and pass 2 skips the row
    /// on its bit (`0x49a494`) — on the next border here, and on the next login, because the mask
    /// is what the character's file carries. Before 2144 the walk read `flags & INITIAL` fresh
    /// and re-joined General on the very next border.
    #[test]
    fn a_left_channel_stays_left_this_session_and_the_next() {
        let mut c = state(SEED);
        c.joined = vec![
            joined("General - Elwynn Forest"),
            suspended("Trade - City"),
            joined("LocalDefense - Elwynn Forest"),
        ];

        // `/leave General`: the stock `SlashCmdList["LEAVE"]` hands `LeaveChannelByName` the bare
        // token and the VM composes it for the zone; `/leave 1` names the slot here.
        let target = c.leave_target("1").expect("slot 1 is confirmed");
        assert_eq!(target, "General - Elwynn Forest", "the slot's wire name");
        c.note_zone_channel_left(&target);
        assert_eq!(c.zone_mask, Some(SEED & !1), "bit 0 cleared");
        // …and the server's YOU_LEFT frees the slot, in place.
        assert_eq!(c.free_slot(&target), Some(1));

        // The next border: General is neither walked (no slot) nor registered (no bit).
        let steps = plan_walk(&c, "Westfall", false, CITY);
        assert_eq!(
            steps,
            vec![
                W::Leave(s("LocalDefense - Elwynn Forest")),
                W::Rename {
                    old: s("LocalDefense - Elwynn Forest"),
                    new: s("LocalDefense - Westfall")
                },
                W::Join(s("LocalDefense - Westfall")),
            ]
        );

        // The next login: a fresh slot array under the file's mask.
        let relog = state(SEED & !1);
        let steps = plan_walk(&relog, "Elwynn Forest", false, CITY);
        assert_eq!(
            steps,
            vec![
                W::Register(s("Trade - City")),
                W::Suspend(s("Trade - City")),
                W::Register(s("LocalDefense - Elwynn Forest")),
                W::Join(s("LocalDefense - Elwynn Forest")),
            ],
            "General is gone for good; Trade is now number 1"
        );

        // …until a `/join General` confirms and sets the bit back.
        let mut relog = relog;
        apply_steps(&steps, &mut relog);
        relog.note_zone_channel_joined("General - Elwynn Forest");
        relog.claim_slot("General - Elwynn Forest");
        assert_eq!(relog.zone_mask, Some(SEED));
        let steps = plan_walk(&relog, "Westfall", false, CITY);
        assert!(
            steps.contains(&W::Join(s("General - Westfall"))),
            "walked again from its slot: {steps:?}"
        );
    }

    /// A mask of zero is a legitimate reference state — a player who left every zone channel —
    /// and it walks nothing. An unseated mask walks nothing either, for the opposite reason: the
    /// file has not been read yet, and the system-level gate keeps `at` unset so the walk runs
    /// once it has.
    #[test]
    fn a_zero_or_unseated_mask_registers_nothing() {
        assert!(plan_walk(&state(0), "Elwynn Forest", false, CITY).is_empty());
        let unseated = ChannelState {
            channels: catalog(),
            zone_mask: None,
            ..Default::default()
        };
        assert!(plan_walk(&unseated, "Elwynn Forest", false, CITY).is_empty());
        assert!(!unseated.zone_row_wanted(1));
    }

    /// **`GuildRecruitment` rides the walk like Trade once its bit is set** — which is the whole
    /// of how the reference keeps an unguilded player in `GuildRecruitment - City` after the
    /// cascade's first join: the confirmed join ORs bit 24, and from then on the
    /// walk registers, suspends and re-joins it through the same city gate (`flags & 0x10`) and
    /// the same city name (`flags & 0x20`). The DBC row order puts it last.
    #[test]
    fn the_guild_recruitment_bit_rides_the_walk_like_trade() {
        let steps = plan_walk(
            &state(SEED | GUILD_RECRUITMENT_BIT),
            "Elwynn Forest",
            false,
            CITY,
        );
        assert_eq!(
            &steps[6..],
            &[
                W::Register(s("GuildRecruitment - City")),
                W::Suspend(s("GuildRecruitment - City")),
            ],
            "registered and suspended out here, number 4"
        );
        let steps = plan_walk(
            &state(SEED | GUILD_RECRUITMENT_BIT),
            "Ironforge",
            true,
            CITY,
        );
        assert_eq!(
            &steps[6..],
            &[
                W::Register(s("GuildRecruitment - City")),
                W::Join(s("GuildRecruitment - City")),
            ]
        );
    }

    /// **A channel joined outside the walk is walked from its slot** — a `/join General` after a
    /// `/leave`, or the cascade's own `GuildRecruitment - City` join. The reference's pass 1 is
    /// the slot array, so it needs no separate record of what the walk asked for; ours used to
    /// keep one (`held`), and a slot it did not know about was registered and joined a second
    /// time on the next border.
    #[test]
    fn a_channel_joined_outside_the_walk_is_walked_from_its_slot() {
        let mut c = state(SEED | GUILD_RECRUITMENT_BIT);
        c.joined = vec![
            joined("General - Stormwind City"),
            joined("Trade - City"),
            joined("LocalDefense - Stormwind City"),
        ];
        // The cascade's join: a slot, then the notice sets the bit.
        c.claim_slot("GuildRecruitment - City");
        let steps = plan_walk(&c, "Stormwind City", true, CITY);
        assert!(
            steps.is_empty(),
            "same zone, every name unchanged, every row eligible — nothing to do: {steps:?}"
        );
        // Walking out suspends it exactly like Trade.
        let steps = plan_walk(&c, "Elwynn Forest", false, CITY);
        assert!(steps.contains(&W::Leave(s("GuildRecruitment - City"))));
        assert!(steps.contains(&W::Suspend(s("GuildRecruitment - City"))));
        assert!(!steps.iter().any(|st| matches!(st, W::Register(_))));
    }

    /// A suspended slot whose name moved re-joins **without a LEAVE** (state ≠ 0 sends none) and
    /// through the rename's `(old == 3) ? 1 : 2` — landing on 1, a plain `YOU_JOINED`.
    #[test]
    fn a_suspended_slot_whose_name_moved_rejoins_without_a_leave() {
        let mut c = state(SEED);
        c.joined = vec![suspended("General - Elwynn Forest")];
        let steps = plan_walk(&c, "Westfall", false, CITY);
        assert_eq!(
            &steps[..2],
            &[
                W::Rename {
                    old: s("General - Elwynn Forest"),
                    new: s("General - Westfall")
                },
                W::Join(s("General - Westfall")),
            ]
        );
        apply_steps(&steps, &mut c);
        assert_eq!(
            c.slot_state("General - Westfall"),
            Some(SlotState::Joined),
            "3 → 1, not 2"
        );
    }

    /// The numeric leg of leave-by-name ([`ChannelState::leave_target`], `0x49ee70` step 1): a
    /// non-zero leading integer names a **confirmed** slot or makes the call a no-op; anything else
    /// is already the wire name, composed by the VM or passed through.
    #[test]
    fn leave_target_resolves_a_number_to_a_confirmed_slot_or_to_nothing() {
        let mut c = state(SEED);
        c.joined = vec![
            joined("General - Elwynn Forest"),
            suspended("Trade - City"),
            None,
            joined("MyChan"),
        ];
        assert_eq!(
            c.leave_target("1").as_deref(),
            Some("General - Elwynn Forest")
        );
        assert_eq!(
            c.leave_target("1abc").as_deref(),
            Some("General - Elwynn Forest"),
            "SStrToInt takes the leading digits"
        );
        assert_eq!(
            c.leave_target("2"),
            None,
            "suspended: state 3 fails `0x49be50`, and the whole call is a no-op"
        );
        assert_eq!(c.leave_target("3"), None, "a hole");
        assert_eq!(c.leave_target("9"), None, "out of range");
        assert_eq!(c.leave_target("-1"), None);
        assert_eq!(c.leave_target("0").as_deref(), Some("0"), "0 is a name");
        assert_eq!(
            c.leave_target("").as_deref(),
            Some(""),
            "`/leave` alone sends an empty name"
        );
        assert_eq!(
            c.leave_target("General - Elwynn Forest").as_deref(),
            Some("General - Elwynn Forest"),
            "a composed name passes through"
        );
        assert_eq!(c.leave_target("mychan").as_deref(), Some("mychan"));

        // …and the mask clear needs a slot carrying the wire name (`0x49f0f4`).
        c.note_zone_channel_left("General - Nowhere");
        assert_eq!(c.zone_mask, Some(SEED), "no slot carries it: no clear");
        c.note_zone_channel_left("General - Elwynn Forest");
        assert_eq!(c.zone_mask, Some(SEED & !1));
    }

    /// The two lists never disagree about *resolvability* — only about the city gate. A missing
    /// city word or an unknown zone drops the row from both, so a slot is never taken for a name
    /// we could not compose.
    #[test]
    fn the_slot_set_and_the_join_set_differ_only_by_the_city_gate() {
        let cat = catalog();
        assert_eq!(
            tracked_channels(&cat, SEED, "Stormwind City", None),
            wanted_channels(&cat, SEED, "Stormwind City", true, None),
            "no city word: the city-NAMED rows are unresolvable, so neither list carries them"
        );
        assert!(
            tracked_channels(&cat, SEED, "", CITY).is_empty(),
            "no zone: nothing zone-dependent composes, so nothing takes a slot either"
        );
    }

    /// **A catalog fed with no zone is not the same thing as no catalog**.
    ///
    /// The VM's three readers all treat an *empty* catalog as "no such built-in channel":
    /// `AddChatWindowChannel` stores the name as a custom channel with id 0, and
    /// `JoinChannelByName` returns `(0, nil)` — the custom leg — and **sends
    /// `CMSG_JOIN_CHANNEL("General")`**. Fed zone-less instead, every zone-dependent row is present
    /// but carries `resolved: None`, which is the reference's own "matched a row, no zone text yet"
    /// leg: store nothing, answer nil, send nothing (`0x4a10d9`, `0x49ece5`).
    ///
    /// That gap is what wrote `CHANNELS / General / LocalDefense` into the director's `Onewarrior`
    /// window block with the zone bits stripped, and it was open on every login until the world
    /// settled — because the catalog used to be fed only once a zone was known.
    #[test]
    fn a_zoneless_catalog_still_carries_every_row() {
        let rows = zone_channel_catalog(&catalog(), "", false, None);
        assert_eq!(rows.len(), 6, "every DBC row is present, zone or no zone");

        let by = |s: &str| rows.iter().find(|r| r.shortcut == s).unwrap().clone();
        assert_eq!(by("General").id, 1, "…and each carries its ChannelID");
        assert_eq!(
            by("General").resolved,
            None,
            "a zone-dependent row with no zone is UNRESOLVED — the nil leg, not the custom leg"
        );
        assert_eq!(by("Trade").resolved, None);
        assert_eq!(
            by("WorldDefense").resolved,
            Some("WorldDefense".to_string()),
            "a row whose name carries no %s resolves with no zone at all"
        );

        // And the moment a zone lands, the same rows resolve.
        let rows = zone_channel_catalog(&catalog(), "Elwynn Forest", false, CITY);
        let by = |s: &str| rows.iter().find(|r| r.shortcut == s).unwrap().clone();
        assert_eq!(
            by("General").resolved,
            Some("General - Elwynn Forest".to_string())
        );
        assert!(
            !by("Trade").listed,
            "…and Trade is city-only, so it is not listed out here"
        );
    }

    /// arg7: the composed names resolve back to their `ChatChannels.dbc` id, custom ones to 0 —
    /// matched by the server's own substring rule.
    #[test]
    fn composed_names_carry_their_channel_id_back() {
        let cat = catalog();
        assert_eq!(cat.zone_channel_id("General - Elwynn Forest"), 1);
        assert_eq!(cat.zone_channel_id("Trade - City"), 2);
        assert_eq!(cat.zone_channel_id("LocalDefense - Durotar"), 22);
        assert_eq!(cat.zone_channel_id("World"), 0);
    }
}
