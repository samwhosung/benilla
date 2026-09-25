//! The instance lockout messages, the bookkeeping behind `CanShowResetInstances()`, and
//! `CMSG_RESET_INSTANCES`. No 1.12 Lua handles the lines: the client fills the `GlobalStrings.lua`
//! template itself and hands it to the chat composer `0x49a870` as `CHAT_MSG_SYSTEM`; here the net
//! handlers queue them and [`feed_instance`] resolves them against the VM.
//!
//! Deviation: walking out of a party dungeon ourselves also counts as holding a bind
//! ([`InstanceState::saw_own_dungeon`]), because vmangos reports only permanent binds
//! (`Player.cpp:16175`) and makes only raid binds permanent (`Map.cpp:3528`), so the server alone
//! would never offer the reset after a 5-man.

use benilla_protocol::messages::{InstanceResetFailure, RaidInstanceMessage, RaidInstanceWarning};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands};
use crate::ui_chat::{ChatEvent, ChatEventKind, ChatLog};
use crate::ui_script::{UiFeed, UiInput};

/// `CanShowResetInstances()`'s window, 25 hours: `cmp eax, 0x15f90` at `0x495ce6` against
/// `time(0)` minus when we left, unsigned, so the boundary is inclusive.
const RESET_OFFER_WINDOW_SECS: u64 = 90_000;

/// The lockout bookkeeping: three of the four globals `CanShowResetInstances` (`0x495c90`) reads;
/// the fourth, the map we stand on (`0xb4e378`), is [`benilla_world::world_map::CurrentMap`].
/// Session-scoped, like the reference's zero-initialized globals.
#[derive(Resource, Default)]
pub(crate) struct InstanceState {
    /// `0xb4e37c`, "do you hold any bind": `SMSG_UPDATE_INSTANCE_OWNERSHIP`, sent on every map
    /// change (`Player.cpp:2116`), where vmangos says yes only for a raid save.
    owns_saved: bool,
    /// benilla's half of the same term: we watched the player walk out of a party dungeon. Raised
    /// by [`LatchWriter::WorldEntry`], cleared by a reset and at logout, and never by an
    /// `owns_saved = false`, which vmangos sends on the very teleport out of the dungeon.
    saw_own_dungeon: bool,
    /// `0xb4e374`: the `Map.dbc` id of the last dungeon left; only a party dungeon lands here
    /// (`0x495d33`).
    last_dungeon: Option<u32>,
    /// `0xb4e370`: when [`Self::last_dungeon`] was recorded, in seconds. Deviation: the app's
    /// monotonic clock, not `time(0)`, because it cannot run backwards; they differ only across a
    /// suspend.
    last_dungeon_at: u64,
    /// `SMSG_UPDATE_LAST_INSTANCE` map ids for [`track_instance_state`]: the packet tests against
    /// the current map, which [`benilla_world::world_map::CurrentMap`] reaches only after the
    /// drain's deferred insert.
    pending_last_instance: Vec<u32>,
    /// Lines for [`feed_instance`] to resolve against the VM's `GlobalStrings.lua`; the wire drain
    /// cannot reach the VM.
    lines: Vec<LockoutLine>,
}

/// One lockout line as the reference holds it between `GetText` and `SStrPrintf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockoutLine {
    /// The `GlobalStrings.lua` token.
    token: &'static str,
    /// `GetText`'s third argument: `Some(n)` picks `_P1` for every `n != 1`; `None` is the
    /// reference's `or edx,-1`, always the bare token.
    ordinal: Option<u32>,
    /// The `Map.dbc` id whose name fills `%s`; `None` for `INSTANCE_SAVED`, which has no `%s`.
    map: Option<u32>,
    /// The `%d` fills in template order (`RAID_INSTANCE_WELCOME` takes three).
    numbers: Vec<u32>,
    /// The `"(Debug-Only Lock Notice) %s"` wrapper, `SMSG_INSTANCE_SAVE_CREATED`'s flag-1 arm
    /// (`0x4e7eb3`).
    debug_notice: bool,
}

/// Which of the last-dungeon latch's two writers is speaking. The reference does not care; benilla
/// does, as only the world entry is first-hand evidence of a bind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LatchWriter {
    /// `0x464ff0`, the map-load path: we just walked out of this map.
    WorldEntry,
    /// `0x49e6ac`, `SMSG_UPDATE_LAST_INSTANCE`: the server's word, already given as `owns_saved`.
    Packet,
}

impl InstanceState {
    /// `SMSG_UPDATE_INSTANCE_OWNERSHIP` → `0x495d50`: one store, no line.
    pub(crate) fn set_ownership(&mut self, owns: bool) {
        self.owns_saved = owns;
    }

    /// The latch `0x495d10`: record `map` as the dungeon just left if it is a party dungeon and
    /// not the map we stand on, tested first (`cmp edx,esi; je`), as vmangos sends one packet per
    /// permanent bind on every map change, the entered instance included.
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
            // Walking out of a 5-man proves the bind the server took on entry.
            if writer == LatchWriter::WorldEntry {
                self.saw_own_dungeon = true;
            }
        }
    }

    /// `SMSG_INSTANCE_RESET` → `0x495d00`, before the body is read: any reset clears the offer.
    pub(crate) fn clear_last_instance(&mut self) {
        self.last_dungeon = None;
        self.last_dungeon_at = 0;
        // The witness goes with the reset; the reference's bind term is the server's to retract.
        self.forget_witness();
    }

    /// Drop benilla's half of the bind term, the one piece of state no server corrects.
    pub(crate) fn forget_witness(&mut self) {
        self.saw_own_dungeon = false;
    }

    /// `CanShowResetInstances()` (`0x495c90`), its four terms in the reference's order:
    ///
    /// 1. a bind held: `0xb4e37c`, or [`InstanceState::saw_own_dungeon`];
    /// 2. not standing in a party dungeon;
    /// 3. the last dungeon left is a party dungeon `Map.dbc` knows;
    /// 4. left no more than [`RESET_OFFER_WINDOW_SECS`] ago.
    ///
    /// A map with no `Map.dbc` row is not a party dungeon (the reference's null-record branch).
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

    /// Queue a line: one per packet, so two identical warnings print twice, as in the reference.
    fn push(&mut self, line: LockoutLine) {
        self.lines.push(line);
    }

    fn take_lines(&mut self) -> Vec<LockoutLine> {
        std::mem::take(&mut self.lines)
    }

    fn take_pending_last_instance(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.pending_last_instance)
    }
}

/// One queued line's text: `GetText`, the `%s` and `%d` fills, then the debug wrapper; `None` for
/// a token the player's `GlobalStrings.lua` lacks or leaves empty, as the reference's null and
/// empty guards do.
fn lockout_text(
    line: &LockoutLine,
    map_name: Option<&str>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    // `GetText(token, nil, ordinal)`'s own plural pick, through the shared primitive.
    let template = benilla_ui::strings::plural(line.token, line.ordinal, get)?;
    // Every lockout template takes its `%s` before its `%d`s, so one ordered list is exact.
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

/// The reference's `"(Debug-Only Lock Notice) %s"` (`0x84c36c`), a literal in the binary, not a
/// GlobalString. Only flag 1 shows it; vmangos sends 0 (`Map.cpp:2289`, `Map.cpp:2389`).
const DEBUG_LOCK_NOTICE_PREFIX: &str = "(Debug-Only Lock Notice) ";

/// A template's `%s`: the `Map.dbc` name, or the id in decimal when there is no row, as the
/// reference prints it (`0x49e228`, `"%d"` at `0x835154`, when `[0xc0daa8][id]` is null).
fn map_name(map: u32, catalog: Option<&benilla_assets::MapCatalogRes>) -> String {
    catalog
        .and_then(|c| c.0.name(map))
        .filter(|n| !n.is_empty())
        .map_or_else(|| map.to_string(), str::to_string)
}

/// The line for one `SMSG_RAID_INSTANCE_MESSAGE`, `None` for a type the jump table drops (0 and 5
/// up, `0x49e246`). The reference divides with truncation (hours `0x49e259`, minutes `0x49e2c8`
/// and `0x49e337`, the welcome's days, hours and minutes `0x49e3a6`), and the plural follows the
/// truncated number.
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

/// The six lockout packet handlers; the feed, which has the VM, resolves the lines they queue.
pub(crate) mod net {
    use super::*;

    use benilla_protocol::messages::InstanceResetFailed;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::RaidInstanceMessage, on_packet)
            .net_handler(K::InstanceSaveCreated, on_packet)
            .net_handler(K::InstanceReset, on_packet)
            .net_handler(K::InstanceResetFailed, on_packet)
            .net_handler(K::UpdateLastInstance, on_packet)
            .net_handler(K::UpdateInstanceOwnership, on_packet);
    }

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

    /// `SMSG_RAID_INSTANCE_MESSAGE` (`0x49e1c0`): one of four `RAID_INSTANCE_*` lines.
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

    /// `SMSG_INSTANCE_RESET` (`0x49e470`): clear the latch (`0x495d00`) before the body is read,
    /// then print `INSTANCE_RESET_SUCCESS` (`0x49e4ea`).
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

    /// `SMSG_INSTANCE_RESET_FAILED` (`0x49e540`): one of three refusals. Deviation: reason 3 and up
    /// prints nothing, where the reference prints an uninitialized 2 KB stack buffer, because that
    /// is a bug and vmangos means 3 and up as silent (`MapPersistentStateMgr.h:275`).
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

    /// `SMSG_UPDATE_LAST_INSTANCE` (`0x49e670`): queued for [`track_instance_state`] to weigh
    /// against the map we end up on. No line.
    pub(crate) fn update_last_instance(state: &mut InstanceState, map: u32) {
        state.pending_last_instance.push(map);
    }

    /// `SMSG_UPDATE_INSTANCE_OWNERSHIP` (`0x49e6c0`): one store (`0x495d50`), no line.
    pub(crate) fn update_instance_ownership(state: &mut InstanceState, owns: bool) {
        state.set_ownership(owns);
    }

    /// `SMSG_INSTANCE_SAVE_CREATED` (`0x4e7e60`): `INSTANCE_SAVED`, bare on flag 0 and wrapped on
    /// flag 1. Deviation: flag 2 and up prints nothing, where the reference prints an
    /// uninitialized 2 KB stack buffer, because that is a bug, not a mechanism.
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

/// Runs both latch writers, VM-free so the latch survives a `/reload` as the reference's globals
/// do. The first observed map records nothing: there is no map left at login, as in the reference
/// (`0xb4e378` starts at 0, and map 0 is not a dungeon).
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

    // Writer 1, the world entry: `prev` is the map we are leaving.
    if let Some(prev) = last_map.replace(here) {
        state.note_last_instance(
            prev,
            Some(here),
            party_dungeon(prev),
            now,
            LatchWriter::WorldEntry,
        );
    }
    // Writer 2, the packet, now that `CurrentMap` has caught up with the transfer.
    for map in state.take_pending_last_instance() {
        let is_dungeon = party_dungeon(map);
        state.note_last_instance(map, Some(here), is_dungeon, now, LatchWriter::Packet);
    }
}

/// Show every queued line as `CHAT_MSG_SYSTEM`, then publish `IsInInstance` and
/// `CanShowResetInstances` every frame, as both move with the map and the clock (the setters diff).
fn feed_instance(
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<InstanceState>,
    mut chat: ResMut<ChatLog>,
    maps: Option<Res<benilla_assets::MapCatalogRes>>,
    current_map: Option<Res<benilla_world::world_map::CurrentMap>>,
    time: Res<Time<Real>>,
) {
    let Some(mut script) = script else {
        // No VM (an engine-only run): drop the queue rather than let it grow; a real session seats
        // the VM before any of these packets can arrive.
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
            // A missing or empty key: silence.
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

/// Drop the witness at logout, as the next login may be another character. The three globals stay,
/// as the reference keeps them (`0x495d00` runs only on `SMSG_INSTANCE_RESET`): its bind term is
/// re-sent at the next world entry, where ours has no server to correct it.
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
                // The latch before the feed, so a map change and its answer share a frame.
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

    /// The shipped 1.12 templates from `GlobalStrings.lua`, so these tests need no install.
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

    #[test]
    fn welcome_line_breaks_the_duration_into_d_h_m() {
        // 3 d 2 h 5 m 30 s: the seconds truncate away.
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

        // Under an hour truncates to 0, and 0 takes `_P1`.
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

    /// The jump table drops type 5 too, which later clients call `RAID_INSTANCE_EXPIRED`.
    #[test]
    fn dropped_warning_types_make_no_line() {
        assert!(warning(0, 409, 60).is_none());
        assert!(warning(5, 409, 60).is_none());
        assert!(warning(u32::MAX, 409, 60).is_none());
    }

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

        // `INSTANCERESET_FAIL_SILENTLY`: no line, where the reference prints a stack buffer.
        let mut state = InstanceState::default();
        net::instance_reset_failed(
            &mut state,
            benilla_protocol::messages::InstanceResetFailed { reason: 3, map: 36 },
        );
        assert!(state.take_lines().is_empty());
    }

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

    /// `GetText`'s own `if ( not string )` arm.
    #[test]
    fn a_missing_plural_twin_falls_back_to_the_bare_token() {
        // Stand-ins, so each assertion reads as which key it reached.
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

    #[test]
    fn an_unknown_map_id_prints_as_its_number() {
        assert_eq!(map_name(9999, None), "9999");
        let line = warning(4, 9999, 3_600).expect("type 4");
        assert_eq!(
            line_text(&line, Some(&map_name(9999, None))).as_deref(),
            Some("Welcome to 9999. This raid instance is scheduled to reset in 0d 1h 0m.")
        );
    }

    #[test]
    fn last_instance_records_only_a_party_dungeon_we_are_not_in() {
        let mut state = InstanceState::default();

        // A raid we left: not recorded (type 2, not 1).
        state.note_last_instance(409, Some(0), false, 100, LatchWriter::Packet);
        assert_eq!(state.last_dungeon, None);

        // The dungeon we stand in: the early-out fires before the type test.
        state.note_last_instance(36, Some(36), true, 100, LatchWriter::Packet);
        assert_eq!(state.last_dungeon, None);

        state.note_last_instance(36, Some(0), true, 100, LatchWriter::Packet);
        assert_eq!(state.last_dungeon, Some(36));
        assert_eq!(state.last_dungeon_at, 100);

        // None of it first-hand: only the world-entry writer is.
        assert!(!state.saw_own_dungeon);
    }

    /// The module's deviation: the witness alone meets term 1, and outlives `owns_saved = false`.
    #[test]
    fn walking_out_of_a_dungeon_satisfies_term_one_by_itself() {
        let dungeons = |m: u32| m == 36;
        let mut state = InstanceState::default();

        // The server's answer alone: no, for every non-raider on vmangos.
        state.set_ownership(false);
        assert!(!state.can_reset(Some(0), &dungeons, 100));

        // We walk out of the Deadmines ourselves.
        state.note_last_instance(36, Some(0), true, 100, LatchWriter::WorldEntry);
        assert!(state.saw_own_dungeon);
        assert!(state.can_reset(Some(0), &dungeons, 100));

        // `SendSavedInstances` says no on that same teleport, and must not undo the witness.
        state.set_ownership(false);
        assert!(state.can_reset(Some(0), &dungeons, 100));

        // The other three terms still hold: the witness replaces term 1 only.
        assert!(!state.can_reset(Some(36), &dungeons, 100)); // 2 · standing back inside
        assert!(!state.can_reset(Some(0), &dungeons, 100 + RESET_OFFER_WINDOW_SECS + 1)); // 4

        // 3 · a reset landed: the latch and the witness go together.
        state.clear_last_instance();
        assert!(!state.saw_own_dungeon);
        assert!(!state.can_reset(Some(0), &dungeons, 100));
    }

    #[test]
    fn the_witness_does_not_survive_a_logout() {
        let dungeons = |m: u32| m == 36;
        let mut state = InstanceState::default();
        state.note_last_instance(36, Some(0), true, 100, LatchWriter::WorldEntry);
        assert!(state.can_reset(Some(0), &dungeons, 100));

        state.forget_witness();
        assert!(!state.can_reset(Some(0), &dungeons, 100));
        // The reference's three globals stay: only our addition is cleared.
        assert_eq!(state.last_dungeon, Some(36));
        assert_eq!(state.last_dungeon_at, 100);
    }

    /// Writer 2 is unreachable against vmangos, which names only permanent binds and makes only
    /// raid binds permanent (`Map.cpp:3528`), so this is its only cover: the queue drains against
    /// the map we end up on, and the packet never raises the witness. Skips without client data.
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

        // One system across ticks: its `Local` holds the last map, and `run_system_once` would
        // make a fresh one each call.
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

        // Writer 1: the first tick only observes map 36; the flip to 0 records it.
        app.update();
        assert_eq!(state(&app), (None, false));

        app.insert_resource(benilla_world::world_map::CurrentMap(0));
        app.update();
        assert_eq!(
            state(&app),
            (Some(36), true),
            "the flip records the dungeon AND our own eyes"
        );

        // Writer 2, on a fresh state: a raid id is dropped, a party dungeon lands without the
        // witness.
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

        // The packet's early-out: the instance we stand in is not one we left.
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
        // A raid is not a party dungeon, so it does not suppress the row.
        assert!(state.can_reset(Some(409), &dungeons, 100));

        // 1 · no bind at all, neither told nor witnessed.
        state.set_ownership(false);
        assert!(!state.saw_own_dungeon);
        assert!(!state.can_reset(Some(0), &dungeons, 100));
        state.set_ownership(true);

        // 3 · the latch cleared by a reset that landed.
        state.clear_last_instance();
        assert!(!state.can_reset(Some(0), &dungeons, 100));
    }

    /// Every token this module emits resolves in the shipped `GlobalStrings.lua` with the fills the
    /// composer expects, and matches the copies quoted above. Skips without client data.
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

        // The three plural twins exist and differ from their bare tokens.
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
        // The welcome has none, which is why the reference passes it no ordinal.
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

        // Two lines end to end against the real strings: the welcome and a refusal.
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
