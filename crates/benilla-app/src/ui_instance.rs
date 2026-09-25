//! Instance/raid **lockout messages** — the six server packets that tell you about a saved
//! instance, the bookkeeping two of them feed, and the one thing you can do about it.
//!
//! Four of the six become a `CHAT_MSG_SYSTEM` line, and **the client composes every one of them
//! itself**: no Lua handler in 1.12 touches this family (`RAID_INSTANCE_WELCOME`,
//! `INSTANCE_RESET_SUCCESS`, `INSTANCE_SAVED` and the rest appear in `GlobalStrings.lua` and
//! nowhere else in FrameXML). Each handler resolves the packet's `Map.dbc` id to a display name,
//! looks the template up through Lua `GetText`, fills it with `SStrPrintf`, and hands the result
//! to the chat chokepoint `0x49a870` with `edx = 0xa` = `CHAT_MSG_SYSTEM`. That whole shape is
//! reproduced here; the only piece the app cannot do alone is the GlobalStrings read, which is why
//! the lines are QUEUED by the net drain and RESOLVED in [`feed_instance`] where the VM is — the
//! questgiver-refusal split of.
//!
//! ## What each packet does (VERIFIED at the bytes, WoW.exe build 5875)
//!
//! | packet | handler | line |
//! |---|---|---|
//! | `SMSG_RAID_INSTANCE_MESSAGE` | `0x49e1c0` | one of four `RAID_INSTANCE_*` templates |
//! | `SMSG_INSTANCE_RESET` | `0x49e470` | `INSTANCE_RESET_SUCCESS`, and clears the last-dungeon latch first |
//! | `SMSG_INSTANCE_RESET_FAILED` | `0x49e540` | one of three `INSTANCE_RESET_FAILED*` templates |
//! | `SMSG_INSTANCE_SAVE_CREATED` | `0x4e7e60` | `INSTANCE_SAVED` |
//! | `SMSG_UPDATE_LAST_INSTANCE` | `0x49e670` | none — records the dungeon we just left |
//! | `SMSG_UPDATE_INSTANCE_OWNERSHIP` | `0x49e6c0` | none — the "I hold a permanent bind" latch |
//!
//! ## The plural rule is the reference's, not ours
//!
//! Three of the four warning templates have a `_P1` twin ("…in %d hour." / "…in %d hours."), and
//! the client picks between them by calling **Lua `GetText(token, nil, ordinal)`** — the binding
//! `0x703bf0` is literally a Lua call into `LocaleProperties.lua`'s `GetText`, pushing the token,
//! then `nil` for gender, then the ordinal (or `nil` when the caller passed `-1`, which is what
//! the three fill-less templates do). `GetPluralIndex` is "1 or absent → the bare token, anything
//! else → `_P1`", and that includes **zero**: `…in 0 hours` is the reference's own output.
//! [`plural_token`] is that rule; the `_P1` probe falls back to the bare token exactly as
//! `GetText`'s own `if ( not string )` arm does.
//!
//! ## Two client bugs deliberately not reproduced
//!
//! `SMSG_INSTANCE_RESET_FAILED` with a reason ≥ 3 and `SMSG_INSTANCE_SAVE_CREATED` with a flag
//! ≥ 2 both fall past every arm of their handler's ladder and then call the chat chokepoint with
//! an **uninitialized 2 KB stack buffer**. vmangos's own enum names reason 3
//! `INSTANCERESET_FAIL_SILENTLY` "as well as any above this", so silence is the server's stated
//! intent; printing stack garbage is fidelity to an accident, not to a mechanism. Both print
//! nothing here.
//!
//! ## `CanShowResetInstances()` is a four-term predicate over `Map.dbc`
//!
//! The reference keeps four globals — `0xb4e370` (when we left), `0xb4e374` (what we left),
//! `0xb4e378` (where we are now) and `0xb4e37c` (do we hold a permanent bind) — and
//! `CanShowResetInstances 0x495c90` reads all four ([`InstanceState::can_reset`]). Three of them
//! live in [`InstanceState`]; the fourth is benilla's own [`benilla_world::world_map::CurrentMap`],
//! which is the same value by construction (the reference writes `0xb4e378` from
//! `[[0xb41414]+0xcc]`, the world's current map, at every world entry and at every
//! `SMSG_UPDATE_LAST_INSTANCE`).
//!
//! **The predicate is narrower than it looks**: `cmp [rec+8],1` means *party dungeon*, so the row
//! is offered only after a 5-man — never after a raid, and never while standing in a 5-man. The
//! 25-hour window (`0x15f90` seconds against `time(0)`, `0x495ce6`) is measured from when the
//! dungeon was left.
//!
//! ## Term 1 is the server's job, and vmangos gets it wrong
//!
//! `0xb4e37c` is written by exactly one function (`0x495d50`) with exactly one caller
//! (`0x49e6d2`, the `SMSG_UPDATE_INSTANCE_OWNERSHIP` handler) — grepped over the whole `.text`,
//! so the client has no other source for it. Its real meaning is **"you hold at least one
//! instance bind"**, and the binary proves that by the company it keeps: the
//! `SMSG_UPDATE_LAST_INSTANCE` handler throws away every map that is not `InstanceType == 1`
//! (`0x495d33`), so a server that only ever named RAID binds in that packet would leave a
//! handler whose only possible effect is nothing. The packet exists to name **5-man** binds.
//!
//! vmangos names none. All three of its senders — `Player::SendRaidInfo`,
//! `Player::SendSavedInstances`, and the `UpdateLastInstance` loop inside it — filter
//! `m_boundInstances` to `itr.second.perm`, and entering a 5-man takes
//! `DungeonMap::Add`'s `player->BindToInstance(state, false)` — a **non-permanent** bind. So a
//! character without a raid save is told `owns_saved = false` forever, and a client that believes
//! it shows the reset row to nobody it was built for. That is a server defect (inherited from
//! MaNGOS, identical in cmangos), not the reference's behaviour.
//!
//! benilla therefore satisfies term 1 from **either** source: the packet, or
//! [`InstanceState::saw_own_dungeon`] — our own eyes. When the world-entry writer below records a
//! party dungeon, it is recording that *we personally just walked out of one*, and the server
//! binds you on entry, so the bind is a fact whether or not vmangos will admit it. This is the
//! one deliberate deviation in this module; everything else is the reference's.
//!
//! ## The last-dungeon latch has TWO writers, and the packet is the lesser one
//!
//! `0x495d10` — "we just moved from map A to map B; if A was a party dungeon, remember it" — is
//! called from two places, and reading only the packet handler gets the feature backwards:
//!
//! 1. **the client's own world entry** (`0x464ff0`, called from the map-load path at `0x4015f3`
//!    and `0x401c22`) with `A` = the map we were on and `B` = the map we are entering. This is
//!    the writer that actually fires when you walk out of the Deadmines;
//! 2. **`SMSG_UPDATE_LAST_INSTANCE`** (`0x49e6ac`) with `A` = the packet's map and `B` = the
//!    world's current map.
//!
//! Against vmangos, **(2) can never record anything** — its `perm` filter (above) means the only
//! maps it ever names are raids, and a raid is `InstanceType == 2`, which `0x495d33`'s
//! `cmp [rec+8],1` rejects. (1) is the only live writer here, which is why it is also the one
//! that answers term 1. (1) is [`track_instance_state`]'s `CurrentMap` flip; (2) is
//! [`net::update_last_instance`]'s queue, drained through the same helper so the two cannot
//! drift — [`LatchWriter`] is which of them is speaking.
//!
//! **(2) is therefore unreachable on our server by construction, and no live probe can cover it**
//! — not with any rigging, because `PermBindAllPlayers` (vmangos's only path to a permanent bind)
//! runs under `if (IsRaid())` and no GM command creates a bind. Its cover is
//! `both_latch_writers_through_the_real_system`, which drives both legs through the real system
//! against the real `Map.dbc`. That is the whole of the evidence behind (2): treat it as tested,
//! never as *observed*.

use benilla_protocol::messages::{InstanceResetFailure, RaidInstanceMessage, RaidInstanceWarning};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands};
use crate::ui_chat::{ChatEvent, ChatEventKind, ChatLog};
use crate::ui_script::{UiFeed, UiInput};

/// The window `CanShowResetInstances()` offers the row in, in seconds — `cmp eax, 0x15f90` at
/// `0x495ce6` against `time(0) - <when we left>`, unsigned, so the boundary is inclusive.
/// 90 000 s = 25 hours.
const RESET_OFFER_WINDOW_SECS: u64 = 90_000;

/// The client's lockout bookkeeping — three of the four globals `CanShowResetInstances 0x495c90`
/// reads (the fourth, the map we are standing on, is [`benilla_world::world_map::CurrentMap`]).
///
/// All of it is session-scoped in the reference too: the globals live in zero-initialized `.data`
/// and nothing persists them, so a fresh login starts with no offer until the server speaks again.
#[derive(Resource, Default)]
pub(crate) struct InstanceState {
    /// `0xb4e37c` — the server's answer to "do you hold any instance bind at all"
    /// (`SMSG_UPDATE_INSTANCE_OWNERSHIP`, sent on every successful map change). **vmangos only
    /// ever says yes to a raid save**, which is the wrong answer for every player this row exists
    /// for — module doc.
    owns_saved: bool,
    /// benilla's own half of the same term: we watched the player walk out of a party dungeon, so
    /// we know they are bound to one without being told.
    ///
    /// Raised by [`LatchWriter::WorldEntry`], cleared by `SMSG_INSTANCE_RESET` and by a **logout**
    /// ([`clear_witness_on_logout`]), and deliberately **not** cleared by an `owns_saved = false`
    /// packet — that packet is the wrong answer this field exists to work around, and vmangos
    /// sends one on the very teleport that takes us out of the dungeon.
    saw_own_dungeon: bool,
    /// `0xb4e374` — the `Map.dbc` id of the dungeon we were last inside, or `None` for "none
    /// recorded". Only a **party dungeon** ever lands here (`0x495d33`'s `cmp [rec+8],1`).
    last_dungeon: Option<u32>,
    /// `0xb4e370` — when [`Self::last_dungeon`] was recorded, in seconds on the same clock
    /// [`Self::can_reset`] compares against. The reference uses `time(0)`; ours is the app's
    /// monotonic real clock, which differs from wall time only across a suspend and cannot run
    /// backwards over a 25-hour window.
    last_dungeon_at: u64,
    /// `SMSG_UPDATE_LAST_INSTANCE` map ids the net drain has queued for [`track_instance_state`].
    /// Queued rather than applied inline because the packet's own test is against the map we are
    /// *currently on*, and benilla writes [`benilla_world::world_map::CurrentMap`] through a
    /// deferred `Commands::insert_resource` — inside the drain it is still the map we left, which
    /// is exactly the value that would make the test answer wrong.
    pending_last_instance: Vec<u32>,
    /// Lines the net drain has queued for [`feed_instance`] to resolve against the VM's own
    /// `GlobalStrings.lua` and show. The reference resolves inline because its handler *is* on the
    /// Lua thread; ours cannot reach the VM from the wire drain (decision 0669's split).
    lines: Vec<LockoutLine>,
}

/// One lockout line, kept the way the reference keeps it between `GetText` and `SStrPrintf`: a
/// GlobalStrings token, the ordinal that picks its plural form, and the fills in the template's
/// own argument order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockoutLine {
    /// The GlobalStrings token (`RAID_INSTANCE_WELCOME`, `INSTANCE_RESET_FAILED_OFFLINE`, …).
    token: &'static str,
    /// `GetText`'s third argument. `Some(n)` selects `_P1` for every `n != 1`; `None` is the
    /// reference's `or edx,-1` — no ordinal, always the bare token.
    ordinal: Option<u32>,
    /// The `Map.dbc` id whose display name fills the template's `%s`, or `None` for a template
    /// with no `%s` (`INSTANCE_SAVED`).
    map: Option<u32>,
    /// The `%d` fills, in the template's own order (`RAID_INSTANCE_WELCOME` takes three).
    numbers: Vec<u32>,
    /// The reference's `"(Debug-Only Lock Notice) %s"` wrapper — the one arm of
    /// `SMSG_INSTANCE_SAVE_CREATED` that does not print its string bare (`0x4e7eb3`).
    debug_notice: bool,
}

/// Which of the last-dungeon latch's two writers is speaking (module doc). The reference shares
/// one function between them and does not care which; benilla does, because only the world-entry
/// writer is first-hand evidence that we are bound to the dungeon it is recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LatchWriter {
    /// `0x464ff0` — the map-load path. We just walked out of this map ourselves.
    WorldEntry,
    /// `0x49e6ac` — `SMSG_UPDATE_LAST_INSTANCE` named the map; the server is telling us about a
    /// bind, and has already said `owns_saved` for it.
    Packet,
}

impl InstanceState {
    /// `SMSG_UPDATE_INSTANCE_OWNERSHIP` → `0x495d50`: one store, no line.
    pub(crate) fn set_ownership(&mut self, owns: bool) {
        self.owns_saved = owns;
    }

    /// `SMSG_UPDATE_LAST_INSTANCE` → `0x495d10`: record `map` as the dungeon we just left, if it
    /// is a party dungeon and is not the map we are standing on.
    ///
    /// The `map == current` early-out is the reference's own first test (`cmp edx,esi; je`), and
    /// it matters: vmangos sends one of these per permanent bind on every map change, so the
    /// instance you are *entering* arrives in the same burst as the ones you are not in.
    pub(crate) fn note_last_instance(
        &mut self,
        map: u32,
        current_map: Option<u32>,
        party_dungeon: bool,
        now_secs: u64,
        writer: LatchWriter,
    ) {
        if current_map == Some(map) {
            return;
        }
        if party_dungeon {
            self.last_dungeon = Some(map);
            self.last_dungeon_at = now_secs;
            // Walking out of a 5-man is first-hand evidence of the bind the server took when we
            // walked in — the half of term 1 vmangos will not give us (module doc).
            if writer == LatchWriter::WorldEntry {
                self.saw_own_dungeon = true;
            }
        }
    }

    /// `SMSG_INSTANCE_RESET` → `0x495d00`, called *before* the body is read: the latch is cleared
    /// whatever the packet says, so the offer goes away the moment any reset lands.
    pub(crate) fn clear_last_instance(&mut self) {
        self.last_dungeon = None;
        self.last_dungeon_at = 0;
        // A reset landed, so whatever we witnessed ourselves is gone with it. The reference has
        // no equivalent line because its term 1 is the server's to retract.
        self.forget_witness();
    }

    /// Drop benilla's own half of term 1 — see [`clear_witness_on_logout`]. Kept a method so the
    /// one piece of state no server will correct has exactly one place that clears it.
    pub(crate) fn forget_witness(&mut self) {
        self.saw_own_dungeon = false;
    }

    /// `CanShowResetInstances()` — the reference's four terms at `0x495c90`, in its own order:
    ///
    /// 1. we hold an instance bind — `0xb4e37c`, **or** benilla's own
    ///    [`InstanceState::saw_own_dungeon`], because vmangos never sets the first one for a
    ///    5-man (module doc);
    /// 2. we are **not** standing in a party dungeon;
    /// 3. the last dungeon we left is a party dungeon that `Map.dbc` knows;
    /// 4. we left it no more than [`RESET_OFFER_WINDOW_SECS`] ago.
    ///
    /// `is_party_dungeon` is the caller's `Map.dbc` lookup (`false` for a map id with no row —
    /// the reference's null-record branch takes the same path as type 0).
    pub(crate) fn can_reset(
        &self,
        current_map: Option<u32>,
        is_party_dungeon: &dyn Fn(u32) -> bool,
        now_secs: u64,
    ) -> bool {
        if !self.owns_saved && !self.saw_own_dungeon {
            return false;
        }
        if current_map.is_some_and(is_party_dungeon) {
            return false;
        }
        let Some(last) = self.last_dungeon else {
            return false;
        };
        if !is_party_dungeon(last) {
            return false;
        }
        now_secs.saturating_sub(self.last_dungeon_at) <= RESET_OFFER_WINDOW_SECS
    }

    /// Queue a line for the feed to resolve. Per PACKET, never per state edge — two identical
    /// warnings ten minutes apart are two lines, exactly as the reference prints two.
    fn push(&mut self, line: LockoutLine) {
        self.lines.push(line);
    }

    /// Take everything queued (the feed's drain).
    fn take_lines(&mut self) -> Vec<LockoutLine> {
        std::mem::take(&mut self.lines)
    }

    /// Take the queued `SMSG_UPDATE_LAST_INSTANCE` map ids ([`track_instance_state`]'s drain).
    fn take_pending_last_instance(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.pending_last_instance)
    }
}

/// Resolve one queued line to its displayed text — `GetText` + the `%s`/`%d` fills, then the
/// debug wrapper if the save-created flag asked for it.
///
/// `None` = show nothing: a token the player's own `GlobalStrings.lua` does not carry, or carries
/// empty, is data-suppression and is answered with silence ([`crate::ui_action::errors::ui_error_text`]'s
/// rule, and the reference's own null/empty guards).
fn lockout_text(
    line: &LockoutLine,
    map_name: Option<&str>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    // `GetText(token, nil, ordinal)`'s own plural pick, through the shared primitive.
    let template = benilla_ui::strings::plural(line.token, line.ordinal, get)?;
    // Arguments in the template's own order — every lockout template takes its `%s` before its
    // `%d`s (checked against all fourteen tokens), which is what makes one ordered list exact.
    let mut args: Vec<benilla_ui::strings::Arg<'_>> = Vec::new();
    args.extend(map_name.map(benilla_ui::strings::Arg::S));
    args.extend(
        line.numbers
            .iter()
            .map(|n| benilla_ui::strings::Arg::D(i64::from(*n))),
    );
    let text = benilla_ui::strings::fill(&template, &args);
    if text.is_empty() {
        return None;
    }
    Some(if line.debug_notice {
        format!("{DEBUG_LOCK_NOTICE_PREFIX}{text}")
    } else {
        text
    })
}

/// The reference's `"(Debug-Only Lock Notice) %s"` (`0x84c36c`), as its literal prefix — a format
/// string compiled into `WoW.exe`, not a GlobalString, so there is no token to look it up by. It
/// is reachable only from `SMSG_INSTANCE_SAVE_CREATED` with flag 1, which vmangos never sends.
const DEBUG_LOCK_NOTICE_PREFIX: &str = "(Debug-Only Lock Notice) ";

/// The name that fills a lockout template's `%s` — `Map.dbc`'s display name, or the map **id** in
/// decimal when the DBC has no row for it.
///
/// The fallback is the reference's own: `0x49e228` and its three twins print the id through
/// `"%d"` (`0x835154`) when `[0xc0daa8][id]` is null or out of range. It is also what the Raid
/// Info panel already does with the same ids.
fn map_name(map: u32, catalog: Option<&benilla_assets::MapCatalogRes>) -> String {
    catalog
        .and_then(|c| c.0.name(map))
        .filter(|n| !n.is_empty())
        .map_or_else(|| map.to_string(), str::to_string)
}

/// Build the line for one `SMSG_RAID_INSTANCE_MESSAGE`, or `None` for a type the reference's jump
/// table drops (0 and ≥ 5, `dec eax; cmp eax,3; ja` at `0x49e246`).
///
/// The three countdown templates take one number, the welcome takes three, and every one of the
/// four divisions is the reference's own integer arithmetic:
///
/// - hours `resetTime / 3600` (`0x49e259`, the `0x91a2b3c5`/`sar 11` magic);
/// - minutes `resetTime / 60` (`0x49e2c8`/`0x49e337`, `0x88888889`/`sar 5`);
/// - welcome `d = t/86400`, `h = (t - 86400d)/3600`, `m = (t - 3600(24d + h))/60` (`0x49e3a6`).
///
/// Truncating division throughout, so "in 1 hour." covers everything from 3600 s to 7199 s — and
/// the *plural* follows the truncated number, not the real duration.
fn raid_instance_line(message: &RaidInstanceMessage) -> Option<LockoutLine> {
    let warning = RaidInstanceWarning::from_wire(message.message_type)?;
    let t = message.reset;
    let (ordinal, numbers) = match warning {
        RaidInstanceWarning::Hours => {
            let hours = t / 3_600;
            (Some(hours), vec![hours])
        }
        RaidInstanceWarning::Minutes | RaidInstanceWarning::MinutesSoon => {
            let mins = t / 60;
            (Some(mins), vec![mins])
        }
        RaidInstanceWarning::Welcome => {
            let days = t / 86_400;
            let hours = (t - days * 86_400) / 3_600;
            let mins = (t - (days * 24 + hours) * 3_600) / 60;
            (None, vec![days, hours, mins])
        }
    };
    Some(LockoutLine {
        token: warning.token(),
        ordinal,
        map: Some(message.map),
        numbers,
        debug_notice: false,
    })
}

/// The instance/raid lockout family's six packet handlers (in the net handler
/// table since 2312), beside the state they drive: four lines the client composes itself out of
/// GlobalStrings, and the two-packet latch behind the SELF menu's reset row. The lines are
/// QUEUED — resolving them needs the VM, which a packet handler leaves to the feed (decision
/// 0669's split).
pub(crate) mod net {
    use super::*;

    use benilla_protocol::messages::InstanceResetFailed;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

    /// Register the six handlers — called from [`UiInstancePlugin`].
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::RaidInstanceMessage, on_packet)
            .net_handler(K::InstanceSaveCreated, on_packet)
            .net_handler(K::InstanceReset, on_packet)
            .net_handler(K::InstanceResetFailed, on_packet)
            .net_handler(K::UpdateLastInstance, on_packet)
            .net_handler(K::UpdateInstanceOwnership, on_packet);
    }

    /// One handler for the six: each is a one-line fold into the same state.
    fn on_packet(In(ev): In<SessionEvent>, mut state: ResMut<InstanceState>) {
        match ev {
            SessionEvent::RaidInstanceMessage { message } => {
                raid_instance_message(&mut state, message)
            }
            SessionEvent::InstanceSaveCreated { flag } => instance_save_created(&mut state, flag),
            SessionEvent::InstanceReset { map } => instance_reset(&mut state, map),
            SessionEvent::InstanceResetFailed { failure } => {
                instance_reset_failed(&mut state, failure)
            }
            SessionEvent::UpdateLastInstance { map } => update_last_instance(&mut state, map),
            SessionEvent::UpdateInstanceOwnership { owns } => {
                update_instance_ownership(&mut state, owns)
            }
            _ => {}
        }
    }

    /// `SMSG_RAID_INSTANCE_MESSAGE` — one of four `RAID_INSTANCE_*` lines (`0x49e1c0`).
    pub(crate) fn raid_instance_message(state: &mut InstanceState, message: RaidInstanceMessage) {
        match raid_instance_line(&message) {
            Some(line) => state.push(line),
            None => debug!(
                "ui_instance: SMSG_RAID_INSTANCE_MESSAGE type {} has no template — silent, as the \
                 reference's jump table is",
                message.message_type
            ),
        }
    }

    /// `SMSG_INSTANCE_RESET` — clear the last-dungeon latch (`0x495d00`, before the body is even
    /// read), then print `INSTANCE_RESET_SUCCESS` (`0x49e4ea`).
    pub(crate) fn instance_reset(state: &mut InstanceState, map: u32) {
        state.clear_last_instance();
        state.push(LockoutLine {
            token: "INSTANCE_RESET_SUCCESS",
            ordinal: None,
            map: Some(map),
            numbers: Vec::new(),
            debug_notice: false,
        });
    }

    /// `SMSG_INSTANCE_RESET_FAILED` — one of three refusals; reason ≥ 3 is silent (module doc).
    pub(crate) fn instance_reset_failed(state: &mut InstanceState, failure: InstanceResetFailed) {
        match InstanceResetFailure::from_wire(failure.reason) {
            Some(reason) => state.push(LockoutLine {
                token: reason.token(),
                ordinal: None,
                map: Some(failure.map),
                numbers: Vec::new(),
                debug_notice: false,
            }),
            None => debug!(
                "ui_instance: SMSG_INSTANCE_RESET_FAILED reason {} is INSTANCERESET_FAIL_SILENTLY \
                 or above — no line",
                failure.reason
            ),
        }
    }

    /// `SMSG_UPDATE_LAST_INSTANCE` — queue the map id for [`track_instance_state`] to weigh
    /// against the map we are actually standing on (module doc, writer 2). No line.
    pub(crate) fn update_last_instance(state: &mut InstanceState, map: u32) {
        state.pending_last_instance.push(map);
    }

    /// `SMSG_UPDATE_INSTANCE_OWNERSHIP` — one store, no line (`0x495d50`).
    pub(crate) fn update_instance_ownership(state: &mut InstanceState, owns: bool) {
        state.set_ownership(owns);
    }

    /// `SMSG_INSTANCE_SAVE_CREATED` — `INSTANCE_SAVED`, bare on flag 0 and wrapped on flag 1
    /// (`0x4e7e60`). Flag ≥ 2 is silent (module doc).
    pub(crate) fn instance_save_created(state: &mut InstanceState, flag: u32) {
        let debug_notice = match flag {
            0 => false,
            1 => true,
            other => {
                debug!("ui_instance: SMSG_INSTANCE_SAVE_CREATED flag {other} has no arm — no line");
                return;
            }
        };
        state.push(LockoutLine {
            token: "INSTANCE_SAVED",
            ordinal: None,
            map: None,
            numbers: Vec::new(),
            debug_notice,
        });
    }
}

/// Keep the client's lockout bookkeeping — the map-change writer and the packet writer of the
/// last-dungeon latch (module doc). Deliberately VM-free: the latch is world state, and it has to
/// keep running through a `/reload` (and in an engine-only harness) exactly as the reference's
/// globals do.
///
/// The `Local` holds the last **observed** map, so the first observation records nothing — there
/// is no map we left at login, which is also what the reference reaches (its `0xb4e378` starts at
/// zero, and Azeroth is `InstanceType` 0). `benilla_world::world_map`'s own `announce_map_change`
/// uses the identical shape for the same reason.
fn track_instance_state(
    mut state: ResMut<InstanceState>,
    maps: Option<Res<benilla_assets::MapCatalogRes>>,
    current_map: Option<Res<benilla_world::world_map::CurrentMap>>,
    time: Res<Time<Real>>,
    mut last_map: Local<Option<u32>>,
) {
    let Some(here) = current_map.as_ref().map(|m| m.0) else {
        return;
    };
    let now = time.elapsed().as_secs();
    let party_dungeon = |m: u32| maps.as_ref().is_some_and(|c| c.0.is_party_dungeon(m));

    // Writer 1 — the world entry. `prev` is the map we are leaving, which is the candidate.
    if let Some(prev) = last_map.replace(here) {
        state.note_last_instance(
            prev,
            Some(here),
            party_dungeon(prev),
            now,
            LatchWriter::WorldEntry,
        );
    }
    // Writer 2 — the packet, now that `CurrentMap` has caught up with the transfer.
    for map in state.take_pending_last_instance() {
        let is_dungeon = party_dungeon(map);
        state.note_last_instance(map, Some(here), is_dungeon, now, LatchWriter::Packet);
    }
}

/// Resolve every queued line against the VM's own `GlobalStrings.lua` and push it as a
/// `CHAT_MSG_SYSTEM` event, then publish the two readers' answers (`IsInInstance`,
/// `CanShowResetInstances`).
///
/// The publish is unconditional and idempotent — both setters diff internally — because both
/// answers move with the map under us and with the clock, neither of which raises an event.
fn feed_instance(
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<InstanceState>,
    mut chat: ResMut<ChatLog>,
    maps: Option<Res<benilla_assets::MapCatalogRes>>,
    current_map: Option<Res<benilla_world::world_map::CurrentMap>>,
    time: Res<Time<Real>>,
) {
    let Some(mut script) = script else {
        // No VM (an engine-only run, a headless probe): there is nowhere to resolve a template
        // and nowhere to show it, so DROP what is queued rather than let it accumulate for a
        // consumer that will never come. Nothing is lost in a real session — the VM is seated
        // long before any of these packets can arrive, and it survives `/reload`.
        let dropped = state.take_lines().len();
        if dropped > 0 {
            debug!("ui_instance: no VM — dropped {dropped} lockout line(s)");
        }
        return;
    };
    let here = current_map.as_ref().map(|m| m.0);
    let now = time.elapsed().as_secs();

    for line in state.take_lines() {
        let name = line.map.map(|m| map_name(m, maps.as_deref()));
        let get = |key: &str| script.lua().globals().get::<String>(key).ok();
        match lockout_text(&line, name.as_deref(), &get) {
            Some(text) => {
                debug!("ui_instance: {} -> {text:?}", line.token);
                chat.push_event(ChatEvent::text_only(ChatEventKind::System, text));
            }
            // The player's own GlobalStrings has no such key (or an empty one) — silence, the way
            // every other GlobalStrings-driven line in this codebase treats a missing key.
            None => debug!("ui_instance: {} resolves to nothing — no line", line.token),
        }
    }

    script.set_instance_type(here.and_then(|m| maps.as_ref().and_then(|c| c.0.instance_type(m))));
    let party_dungeon = |m: u32| maps.as_ref().is_some_and(|c| c.0.is_party_dungeon(m));
    script.set_can_reset_instances(state.can_reset(here, &party_dungeon, now));
}

/// Turn the `ResetInstances()` calls the dialog made into `CMSG_RESET_INSTANCES` sends.
fn drain_instance(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for _ in 0..script.take_reset_instance_asks() {
        let _ = commands.0.send(ClientCommand::ResetInstances);
    }
}

/// The lockout family: the four chat lines, the bookkeeping behind the SELF menu's reset row, and
/// that row's one send.
/// Drop the witness at logout — the next login is somebody else's character (`poi_marker`'s own
/// shape, and the same reason).
///
/// Only [`InstanceState::saw_own_dungeon`] is cleared, not the reference's three globals: the
/// reference does not clear those either (`0x495d00`'s only caller is the `SMSG_INSTANCE_RESET`
/// handler), and it does not need to — its term 1 is the server's, re-advertised on the next
/// character's world entry, so a stale latch cannot offer that character a reset on its own. Ours
/// is the half no server will correct, so it gets the lifetime the packet would have given it.
fn clear_witness_on_logout(
    mut state: ResMut<InstanceState>,
    mut logged_out: MessageReader<crate::net::LoggedOutMessage>,
) {
    if logged_out.read().next().is_some() {
        state.forget_witness();
    }
}

pub(crate) struct UiInstancePlugin;

impl Plugin for UiInstancePlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<InstanceState>().add_systems(
            Update,
            (
                // The latch first: `feed_instance` publishes `CanShowResetInstances()` off it, and
                // a map change and its answer should not be a frame apart.
                clear_witness_on_logout.before(track_instance_state),
                track_instance_state.before(feed_instance),
                feed_instance.in_set(UiFeed),
                drain_instance.after(UiInput),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real 1.12 templates, quoted from `Interface\FrameXML\GlobalStrings.lua` (verified
    /// extract) — the tests resolve against these rather than against whatever the running
    /// player's chain happens to carry.
    fn globals(key: &str) -> Option<String> {
        let v = match key {
            "RAID_INSTANCE_WARNING_HOURS" => "WARNING! %s is scheduled to reset in %d hour.",
            "RAID_INSTANCE_WARNING_HOURS_P1" => "WARNING! %s is scheduled to reset in %d hours.",
            "RAID_INSTANCE_WARNING_MIN" => "WARNING! %s is scheduled to reset in %d minute!",
            "RAID_INSTANCE_WARNING_MIN_P1" => "WARNING! %s is scheduled to reset in %d minutes!",
            "RAID_INSTANCE_WARNING_MIN_SOON" => {
                "WARNING! %s is scheduled to reset in %d minute. Please exit the zone or you will \
                 be returned to your bind location!"
            }
            "RAID_INSTANCE_WARNING_MIN_SOON_P1" => {
                "WARNING! %s is scheduled to reset in %d minutes. Please exit the zone or you will \
                 be returned to your bind location!"
            }
            "RAID_INSTANCE_WELCOME" => {
                "Welcome to %s. This raid instance is scheduled to reset in %dd %dh %dm."
            }
            "INSTANCE_RESET_SUCCESS" => "%s has been reset.",
            "INSTANCE_RESET_FAILED" => {
                "Cannot reset %s.  There are players still inside the instance."
            }
            "INSTANCE_RESET_FAILED_OFFLINE" => {
                "Cannot reset %s.  There are players offline in your party."
            }
            "INSTANCE_RESET_FAILED_ZONING" => {
                "Cannot reset %s.  There are players in your party attempting to zone into an \
                 instance."
            }
            "INSTANCE_SAVED" => "You are now saved to this instance",
            _ => return None,
        };
        Some(v.to_string())
    }

    fn line_text(line: &LockoutLine, name: Option<&str>) -> Option<String> {
        lockout_text(line, name, &globals)
    }

    fn warning(message_type: u32, map: u32, reset: u32) -> Option<LockoutLine> {
        raid_instance_line(&RaidInstanceMessage {
            message_type,
            map,
            reset,
        })
    }

    /// The welcome line: three truncated fills in `d`/`h`/`m` order, and NO plural — the
    /// reference passes `-1` for the ordinal, so `RAID_INSTANCE_WELCOME` is used bare (it has no
    /// `_P1` twin in `GlobalStrings.lua` either).
    #[test]
    fn welcome_line_breaks_the_duration_into_d_h_m() {
        // 3 d 2 h 5 m 30 s — the seconds tail truncates away.
        let secs = 3 * 86_400 + 2 * 3_600 + 5 * 60 + 30;
        let line = warning(4, 409, secs).expect("type 4 has a template");
        assert_eq!(line.token, "RAID_INSTANCE_WELCOME");
        assert_eq!(line.ordinal, None, "the reference passes -1 here");
        assert_eq!(line.numbers, vec![3, 2, 5]);
        assert_eq!(
            line_text(&line, Some("Molten Core")).as_deref(),
            Some("Welcome to Molten Core. This raid instance is scheduled to reset in 3d 2h 5m.")
        );
    }

    /// The three countdown templates take ONE number, and the plural follows that truncated
    /// number: 1 hour is singular, 2 hours and — the case a hand-rolled rule always gets wrong —
    /// **0 hours** are plural.
    #[test]
    fn countdown_lines_pluralize_on_the_truncated_count() {
        let one = warning(1, 409, 3_600 + 59).expect("type 1");
        assert_eq!(one.numbers, vec![1]);
        assert_eq!(
            line_text(&one, Some("Molten Core")).as_deref(),
            Some("WARNING! Molten Core is scheduled to reset in 1 hour.")
        );

        let two = warning(1, 409, 2 * 3_600).expect("type 1");
        assert_eq!(
            line_text(&two, Some("Molten Core")).as_deref(),
            Some("WARNING! Molten Core is scheduled to reset in 2 hours.")
        );

        // Under an hour, the HOURS template still truncates to zero — and zero takes `_P1`.
        let zero = warning(1, 409, 900).expect("type 1");
        assert_eq!(zero.numbers, vec![0]);
        assert_eq!(
            line_text(&zero, Some("Molten Core")).as_deref(),
            Some("WARNING! Molten Core is scheduled to reset in 0 hours.")
        );

        let mins = warning(2, 309, 5 * 60).expect("type 2");
        assert_eq!(
            line_text(&mins, Some("Zul'Gurub")).as_deref(),
            Some("WARNING! Zul'Gurub is scheduled to reset in 5 minutes!")
        );

        let soon = warning(3, 309, 60).expect("type 3");
        assert_eq!(
            line_text(&soon, Some("Zul'Gurub")).as_deref(),
            Some(
                "WARNING! Zul'Gurub is scheduled to reset in 1 minute. Please exit the zone or \
                 you will be returned to your bind location!"
            )
        );
    }

    /// Types the reference's jump table drops print nothing at all — including 5, which later
    /// clients call `RAID_INSTANCE_EXPIRED`.
    #[test]
    fn dropped_warning_types_make_no_line() {
        assert!(warning(0, 409, 60).is_none());
        assert!(warning(5, 409, 60).is_none());
        assert!(warning(u32::MAX, 409, 60).is_none());
    }

    /// The reset lines: success takes the name, each refusal takes the name, and reason ≥ 3 never
    /// reaches a line at all (that arm is in [`apply`], tested through the state).
    #[test]
    fn reset_lines_fill_the_map_name() {
        let mut state = InstanceState::default();
        net::instance_reset(&mut state, 36);
        let lines = state.take_lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(
            line_text(&lines[0], Some("Deadmines")).as_deref(),
            Some("Deadmines has been reset.")
        );

        for (reason, expected) in [
            (
                0u32,
                "Cannot reset Deadmines.  There are players still inside the instance.",
            ),
            (
                1,
                "Cannot reset Deadmines.  There are players offline in your party.",
            ),
            (
                2,
                "Cannot reset Deadmines.  There are players in your party attempting to zone into \
                 an instance.",
            ),
        ] {
            let mut state = InstanceState::default();
            net::instance_reset_failed(
                &mut state,
                benilla_protocol::messages::InstanceResetFailed { reason, map: 36 },
            );
            let lines = state.take_lines();
            assert_eq!(lines.len(), 1, "reason {reason} makes one line");
            assert_eq!(
                line_text(&lines[0], Some("Deadmines")).as_deref(),
                Some(expected)
            );
        }

        // INSTANCERESET_FAIL_SILENTLY and above: no line, rather than the reference's
        // uninitialized-buffer print.
        let mut state = InstanceState::default();
        net::instance_reset_failed(
            &mut state,
            benilla_protocol::messages::InstanceResetFailed { reason: 3, map: 36 },
        );
        assert!(state.take_lines().is_empty());
    }

    /// `SMSG_INSTANCE_SAVE_CREATED`: flag 0 prints the string bare, flag 1 wraps it in the
    /// binary's own debug literal, and anything above prints nothing.
    #[test]
    fn save_created_has_three_arms() {
        let mut state = InstanceState::default();
        net::instance_save_created(&mut state, 0);
        let lines = state.take_lines();
        assert_eq!(
            line_text(&lines[0], None).as_deref(),
            Some("You are now saved to this instance")
        );

        net::instance_save_created(&mut state, 1);
        let lines = state.take_lines();
        assert_eq!(
            line_text(&lines[0], None).as_deref(),
            Some("(Debug-Only Lock Notice) You are now saved to this instance")
        );

        net::instance_save_created(&mut state, 2);
        assert!(state.take_lines().is_empty());
    }

    /// A token the player's `GlobalStrings.lua` does not carry is silence, not a raw token or an
    /// empty chat line.
    #[test]
    fn an_unresolvable_token_makes_no_line() {
        let line = LockoutLine {
            token: "NOT_A_REAL_GLOBAL_STRING",
            ordinal: None,
            map: Some(409),
            numbers: Vec::new(),
            debug_notice: false,
        };
        assert_eq!(line_text(&line, Some("Molten Core")), None);
    }

    /// The `_P1` probe falls back to the bare token when the twin is missing — `GetText`'s own
    /// `if ( not string )` arm. Modelled with a locale that has the singular only.
    #[test]
    fn a_missing_plural_twin_falls_back_to_the_bare_token() {
        // Marked stand-ins rather than the shipped wording: the primitive returns a *template*,
        // so the assertion has to be readable as "which key did it reach", which is the thing
        // worth asserting.
        let sparse = |key: &str| match key {
            "RAID_INSTANCE_WARNING_HOURS" => Some("<bare>".to_string()),
            _ => None,
        };
        let both = |key: &str| match key {
            "RAID_INSTANCE_WARNING_HOURS" => Some("<bare>".to_string()),
            "RAID_INSTANCE_WARNING_HOURS_P1" => Some("<twin>".to_string()),
            "RAID_INSTANCE_WELCOME" => Some("<welcome>".to_string()),
            _ => None,
        };
        let plural = benilla_ui::strings::plural;
        assert_eq!(
            plural("RAID_INSTANCE_WARNING_HOURS", Some(5), &sparse).as_deref(),
            Some("<bare>"),
            "no twin: GetText's own `if ( not string )` arm"
        );
        assert_eq!(
            plural("RAID_INSTANCE_WARNING_HOURS", Some(5), &both).as_deref(),
            Some("<twin>")
        );
        assert_eq!(
            plural("RAID_INSTANCE_WARNING_HOURS", Some(1), &both).as_deref(),
            Some("<bare>"),
            "exactly 1 is the bare token"
        );
        assert_eq!(
            plural("RAID_INSTANCE_WELCOME", None, &both).as_deref(),
            Some("<welcome>"),
            "no ordinal is the bare token"
        );
    }

    /// A map id `Map.dbc` cannot name prints the id, the way the reference's `"%d"` fallback does
    /// (and the way the Raid Info panel already does with the same ids).
    #[test]
    fn an_unknown_map_id_prints_as_its_number() {
        assert_eq!(map_name(9999, None), "9999");
        let line = warning(4, 9999, 3_600).expect("type 4");
        assert_eq!(
            line_text(&line, Some(&map_name(9999, None))).as_deref(),
            Some("Welcome to 9999. This raid instance is scheduled to reset in 0d 1h 0m.")
        );
    }

    /// `SMSG_UPDATE_LAST_INSTANCE` records only a **party dungeon**, and only one that is not the
    /// map we are standing on — the reference's `cmp edx,esi; je` early-out, which matters because
    /// vmangos sends one of these per bind on every map change.
    #[test]
    fn last_instance_records_only_a_party_dungeon_we_are_not_in() {
        let mut state = InstanceState::default();

        // A raid we left: not recorded (type 2, not 1).
        state.note_last_instance(409, Some(0), false, 100, LatchWriter::Packet);
        assert_eq!(state.last_dungeon, None);

        // The dungeon we are standing IN: the early-out fires before the type test.
        state.note_last_instance(36, Some(36), true, 100, LatchWriter::Packet);
        assert_eq!(state.last_dungeon, None);

        // A dungeon we left: recorded, with the clock.
        state.note_last_instance(36, Some(0), true, 100, LatchWriter::Packet);
        assert_eq!(state.last_dungeon, Some(36));
        assert_eq!(state.last_dungeon_at, 100);

        // …and none of that was first-hand: only the world-entry writer is (next test).
        assert!(!state.saw_own_dungeon);
    }

    /// Term 1 without the server's help — decision 1754, the one deliberate deviation here.
    ///
    /// vmangos never sends `owns_saved` for a 5-man bind, so walking out of the Deadmines has to
    /// be enough on its own. It is: the world-entry writer raises
    /// [`InstanceState::saw_own_dungeon`], and the `owns_saved = false` that vmangos sends on that
    /// very teleport must not take it away again.
    #[test]
    fn walking_out_of_a_dungeon_satisfies_term_one_by_itself() {
        let dungeons = |m: u32| m == 36;
        let mut state = InstanceState::default();

        // The server's answer alone: no. This is every non-raider on vmangos.
        state.set_ownership(false);
        assert!(!state.can_reset(Some(0), &dungeons, 100));

        // We walk out of the Deadmines ourselves. That is the bind, whoever admits it.
        state.note_last_instance(36, Some(0), true, 100, LatchWriter::WorldEntry);
        assert!(state.saw_own_dungeon);
        assert!(state.can_reset(Some(0), &dungeons, 100));

        // vmangos sends `SendSavedInstances` on that same far teleport, saying no. It must not
        // undo what we saw — if it did, the row would die on the way out of every dungeon.
        state.set_ownership(false);
        assert!(state.can_reset(Some(0), &dungeons, 100));

        // The other three terms still bind: our own eyes replace term 1, not the predicate.
        assert!(!state.can_reset(Some(36), &dungeons, 100)); // 2 · standing back inside
        assert!(!state.can_reset(Some(0), &dungeons, 100 + RESET_OFFER_WINDOW_SECS + 1)); // 4

        // 3 · a reset landed: the latch AND the witness go together.
        state.clear_last_instance();
        assert!(!state.saw_own_dungeon);
        assert!(!state.can_reset(Some(0), &dungeons, 100));
    }

    /// The witness does not outlive the character that earned it (`clear_witness_on_logout`).
    ///
    /// The reference can leave its stale latch lying about because term 1 is the server's and the
    /// next character's world entry re-answers it. Ours is the half nothing corrects, so a logout
    /// has to, or character B is offered a reset that character A walked out of.
    #[test]
    fn the_witness_does_not_survive_a_logout() {
        let dungeons = |m: u32| m == 36;
        let mut state = InstanceState::default();
        state.note_last_instance(36, Some(0), true, 100, LatchWriter::WorldEntry);
        assert!(state.can_reset(Some(0), &dungeons, 100));

        state.forget_witness();
        assert!(!state.can_reset(Some(0), &dungeons, 100));
        // The reference's own three globals are untouched — we clear our addition, not its state.
        assert_eq!(state.last_dungeon, Some(36));
        assert_eq!(state.last_dungeon_at, 100);
    }

    /// **Both latch writers, driven through the real system, against the real `Map.dbc`.**
    ///
    /// This is the only cover writer 2 — `SMSG_UPDATE_LAST_INSTANCE` — can have. vmangos can
    /// never exercise it: `SendSavedInstances` names only `perm` binds, `DungeonMap::Add` takes a
    /// non-perm bind for a 5-man, and `PermBindAllPlayers` (the one path that makes a bind
    /// permanent) runs under `if (IsRaid())`, so the only map ids that packet can ever carry are
    /// raids — which `0x495d33`'s `cmp [rec+8],1` throws away. No GM command creates a bind
    /// either (`.instance` offers listbinds/unbind/groupunbind, nothing that binds). So the packet
    /// leg is unreachable on our server by construction, and a live probe cannot cover it however
    /// the character is rigged (decision 1754's open thread).
    ///
    /// It runs both legs through `track_instance_state` itself rather than the methods, because
    /// the thing worth guarding is the wiring: that the queue drains against the map we ended up
    /// on rather than the one we left, and that the packet writer does **not** raise the witness
    /// (which would hand 1754's deviation to a server's say-so). Skips without client data.
    #[test]
    fn both_latch_writers_through_the_real_system() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let maps = benilla_formats::load_map_catalog(&mut chain).expect("Map.dbc");
        assert!(maps.is_party_dungeon(36), "Deadmines is InstanceType 1");
        assert!(
            !maps.is_party_dungeon(409),
            "Molten Core is a raid, not a dungeon"
        );

        // The system carries the last observed map in a `Local`, so it has to be the SAME system
        // across ticks — `run_system_once` hands out a fresh one each call and would never see a
        // transition at all.
        let mut app = App::new();
        app.init_resource::<InstanceState>()
            .init_resource::<Time<Real>>()
            .insert_resource(benilla_assets::MapCatalogRes(maps))
            .insert_resource(benilla_world::world_map::CurrentMap(36))
            .add_systems(Update, track_instance_state);

        let state = |app: &App| {
            let s = app.world().resource::<InstanceState>();
            (s.last_dungeon, s.saw_own_dungeon)
        };
        let queue = |app: &mut App, map: u32| {
            net::update_last_instance(&mut app.world_mut().resource_mut::<InstanceState>(), map);
        };

        // Writer 1 — we walk out of the Deadmines ourselves. The first tick only OBSERVES map 36
        // (there is no map we left at login), so nothing is recorded until the flip.
        app.update();
        assert_eq!(state(&app), (None, false));

        app.insert_resource(benilla_world::world_map::CurrentMap(0));
        app.update();
        assert_eq!(
            state(&app),
            (Some(36), true),
            "the flip records the dungeon AND our own eyes (1754's term 1)"
        );

        // Writer 2 — the packet, on a fresh state. A raid id is dropped (the only thing vmangos
        // could ever send); a party dungeon lands, and lands WITHOUT the witness.
        app.insert_resource(InstanceState::default());
        queue(&mut app, 409);
        app.update();
        assert_eq!(state(&app), (None, false), "a raid is not a party dungeon");

        queue(&mut app, 36);
        app.update();
        assert_eq!(
            state(&app),
            (Some(36), false),
            "the packet is the server's word, never our own eyes"
        );

        // …and the packet's own early-out: the instance we are STANDING IN is not one we left.
        app.insert_resource(InstanceState::default());
        app.insert_resource(benilla_world::world_map::CurrentMap(36));
        app.update();
        queue(&mut app, 36);
        app.update();
        assert_eq!(state(&app), (None, false));
    }

    /// `CanShowResetInstances()`'s four terms, one at a time.
    #[test]
    fn can_reset_needs_all_four_terms() {
        let dungeons = |m: u32| m == 36 || m == 33;
        let mut state = InstanceState::default();
        state.set_ownership(true);
        state.note_last_instance(36, Some(0), true, 100, LatchWriter::Packet);

        // All four hold: bound somewhere, standing outside, left a dungeon, recently.
        assert!(state.can_reset(Some(0), &dungeons, 100));
        assert!(state.can_reset(Some(0), &dungeons, 100 + RESET_OFFER_WINDOW_SECS));

        // 4 · past the window.
        assert!(!state.can_reset(Some(0), &dungeons, 100 + RESET_OFFER_WINDOW_SECS + 1));

        // 2 · standing in a party dungeon.
        assert!(!state.can_reset(Some(33), &dungeons, 100));
        // …but a RAID is not a party dungeon, so it does not suppress the row.
        assert!(state.can_reset(Some(409), &dungeons, 100));

        // 1 · no bind at all — neither told nor witnessed (the witness has its own test).
        state.set_ownership(false);
        assert!(!state.saw_own_dungeon);
        assert!(!state.can_reset(Some(0), &dungeons, 100));
        state.set_ownership(true);

        // 3 · the latch cleared by a reset that landed.
        state.clear_last_instance();
        assert!(!state.can_reset(Some(0), &dungeons, 100));
    }

    /// The RUNTIME leg on the real client data (`ui_quest`'s pattern): every token this module can
    /// emit — the four warnings, their three `_P1` twins, the three refusals, the reset success
    /// and `INSTANCE_SAVED` — resolves to a non-empty string in the shipped 1.12
    /// `GlobalStrings.lua`, and carries the fills the composer expects to put in it.
    ///
    /// This is the guard the module-local tests above cannot be: they resolve against the
    /// templates QUOTED in this file, so a typo'd token would still pass every one of them and
    /// degrade a real line to silence in the app. It also re-proves the quoted set is the shipped
    /// set. Skips without client data.
    #[test]
    fn every_lockout_token_resolves_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let real = |key: &str| s.lua().globals().get::<String>(key).ok();

        // Every token, and the quoted copy above is the shipped text.
        for token in [
            "RAID_INSTANCE_WARNING_HOURS",
            "RAID_INSTANCE_WARNING_MIN",
            "RAID_INSTANCE_WARNING_MIN_SOON",
            "RAID_INSTANCE_WELCOME",
            "INSTANCE_RESET_SUCCESS",
            "INSTANCE_RESET_FAILED",
            "INSTANCE_RESET_FAILED_OFFLINE",
            "INSTANCE_RESET_FAILED_ZONING",
            "INSTANCE_SAVED",
        ] {
            let text = real(token).unwrap_or_default();
            assert!(!text.is_empty(), "{token} missing from GlobalStrings.lua");
            assert_eq!(
                Some(text),
                globals(token),
                "{token}: the quoted copy in this file has drifted from the shipped string"
            );
        }

        // The three plural twins exist, so the pick really has something to select — asserted by
        // the twin's own shipped text, which differs from the bare token's.
        for token in [
            "RAID_INSTANCE_WARNING_HOURS",
            "RAID_INSTANCE_WARNING_MIN",
            "RAID_INSTANCE_WARNING_MIN_SOON",
        ] {
            let twin = real(&format!("{token}_P1"));
            assert!(twin.is_some(), "{token} has a shipped plural twin");
            assert_eq!(
                benilla_ui::strings::plural(token, Some(2), &real),
                twin,
                "{token} at two takes its twin"
            );
            assert_ne!(twin, real(token), "{token}'s two forms differ");
        }
        // …and the welcome does not, which is why the reference passes it no ordinal at all.
        assert!(
            real("RAID_INSTANCE_WELCOME_P1").is_none(),
            "RAID_INSTANCE_WELCOME has no _P1 in 1.12"
        );

        // Every warning template names the instance and takes exactly the fills we hand it.
        for (ty, fills) in [(1u32, 1usize), (2, 1), (3, 1), (4, 3)] {
            let line = warning(ty, 409, 3_600).expect("a template");
            let template = benilla_ui::strings::plural(line.token, line.ordinal, &real).unwrap();
            assert!(template.contains("%s"), "{} names the instance", line.token);
            assert_eq!(
                template.matches("%d").count(),
                fills,
                "{} takes {fills} number(s)",
                line.token
            );
            assert_eq!(line.numbers.len(), fills);
        }

        // Two lines end to end against the real strings — the welcome and a refusal.
        let welcome = warning(4, 409, 3 * 86_400 + 2 * 3_600 + 5 * 60).expect("type 4");
        assert_eq!(
            lockout_text(&welcome, Some("Molten Core"), &real).as_deref(),
            Some("Welcome to Molten Core. This raid instance is scheduled to reset in 3d 2h 5m.")
        );
        let mut state = InstanceState::default();
        net::instance_reset_failed(
            &mut state,
            benilla_protocol::messages::InstanceResetFailed { reason: 0, map: 36 },
        );
        assert_eq!(
            lockout_text(&state.take_lines()[0], Some("Deadmines"), &real).as_deref(),
            Some("Cannot reset Deadmines.  There are players still inside the instance.")
        );
    }

    /// The fill is positional and stops at the arguments it has: a template with more specifiers
    /// than fills keeps the leftovers literally rather than eating the next argument.
    #[test]
    fn fill_is_positional_and_never_borrows_the_wrong_argument() {
        use benilla_ui::strings::{fill, Arg};
        assert_eq!(
            fill("%s: %d/%d", &[Arg::S("MC"), Arg::D(2), Arg::D(5)]),
            "MC: 2/5"
        );
        assert_eq!(fill("%s: %d/%d", &[Arg::S("MC"), Arg::D(2)]), "MC: 2/%d");
        assert_eq!(fill("100%% sure", &[]), "100% sure");
        assert_eq!(fill("no fills", &[Arg::D(7)]), "no fills");
    }
}
