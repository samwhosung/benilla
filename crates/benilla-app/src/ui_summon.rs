//! Being summoned: `SMSG_SUMMON_REQUEST` (handler `0x5e6140`) latches the question and fires
//! `CONFIRM_SUMMON` with no arguments; Accept sends `CMSG_SUMMON_RESPONSE` with the summoner's
//! guid. Declining sends nothing: 1.12 has no `CancelSummon`, and the server's two-minute expiry
//! refuses.
//!
//! - The latch (`0x4963a0`) writes four globals, the guid's two halves, the zone and an absolute
//!   deadline, then a bare `SignalEvent(516)`.
//! - A dead or ghost player is refused before the latch (`0x5e6194`), so a live question is
//!   untouched. The test is `0x605f30`: health <= 0, or a player with the `PLAYER_FLAGS` ghost bit
//!   (a ghost's health is 1). With no player object, it latches.
//! - `GetSummonConfirmSummoner()` (`0x48b6a0`) answers `""` on a cache miss and queries the name
//!   (`0x55f1fa`); the popup re-reads it every frame (`StaticPopup.lua:1746`) until it lands.
//! - `ConfirmSummon()` (`0x48b770`) checks no deadline, so two Accepts are two packets; vmangos
//!   ignores the guid and checks its own `m_summon_expire`.
//!
//! Deviation: the latch is dropped at session end and an Accept with nothing latched sends
//! nothing, so no stale or zero guid reaches the server. The reference never clears the guid or
//! zone, only the deadline (`0x49014a`), so its `ConfirmSummon()` sends whatever is left.

use benilla_ui::script::{SummonConfirmUiState, UiScript};
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_script::{UiFeed, UiInput};

/// The latched question, the reference's four globals: written by [`net::request`], read by
/// [`feed_summon`] and [`drain_summon`].
#[derive(Resource, Default)]
pub(crate) struct SummonState {
    /// `[0xb4e358]`/`[0xb4e35c]`, echoed by Accept; 0 when nothing has asked this session.
    summoner: u64,
    /// `[0xb4e354]`: the summoner's `AreaTable` id, not ours and not a map id.
    zone: u32,
    /// `[0xb4e350]`, an absolute ms stamp there; here `Time<Real>` seconds, as the server's window
    /// runs in real time. `None` is the zeroed deadline.
    expires_at: Option<f64>,
    /// A dialog still owed, set per packet: the same warlock summoning twice is two dialogs.
    ask: bool,
}

impl SummonState {
    /// The latch, `0x4963a0`.
    fn latch(&mut self, summoner: u64, zone: u32, delay_ms: u32, now: f64) {
        self.summoner = summoner;
        self.zone = zone;
        self.expires_at = Some(now + f64::from(delay_ms) / 1000.0);
        self.ask = true;
    }

    fn pending(&self) -> Option<u64> {
        (self.summoner != 0).then_some(self.summoner)
    }

    /// `0x4963e0`: ms left, clamped at zero; the binding (`0x48b660`) truncates to seconds.
    fn time_left_ms(&self, now: f64) -> u32 {
        self.expires_at.map_or(0.0, |at| {
            ((at - now).max(0.0) * 1000.0).min(f64::from(u32::MAX))
        }) as u32
    }
}

/// Push the three getters' answers every frame, and fire `CONFIRM_SUMMON` (no arguments) for an
/// owed question.
fn feed_summon(
    script: Option<NonSendMut<UiScript>>,
    mut summon: ResMut<SummonState>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    areas: Option<Res<AreaTableRes>>,
    // Real: the deadline was stamped on this clock.
    time: Res<Time<Real>>,
) {
    let Some(mut script) = script else {
        return;
    };

    // The getters' defaults already answer a zeroed bank. Not `pending()`: a zero-guid request
    // still latches and fires in the reference, and must not leave `ask` owed.
    if summon.summoner == 0 && !summon.ask {
        return;
    }

    // A miss asks and reports `""` this frame; a zero guid names nobody and is never asked about.
    let summoner_name = match summon.pending() {
        Some(guid) => names
            .resolve(guid, &commands)
            .unwrap_or_default()
            .to_string(),
        None => String::new(),
    };
    script.set_summon_confirm(SummonConfirmUiState {
        summoner: summoner_name,
        area: area_name(summon.zone, areas.as_deref()),
        time_left_ms: summon.time_left_ms(time.elapsed_secs_f64()),
    });

    if !summon.ask {
        return;
    }
    summon.ask = false;
    // The `summon` trace's middle link, between the packet and the response.
    if benilla_assets::trace::enabled_for("summon") {
        benilla_assets::trace::line(
            "summon",
            &format!(
                "fire CONFIRM_SUMMON summoner={:#x} zone={}",
                summon.summoner, summon.zone
            ),
        );
    }
    script.fire_event("CONFIRM_SUMMON", Vec::new());
}

/// `GetSummonConfirmAreaName()` (`0x48b720`): the summoner's bare `AreaTable.dbc` row name or
/// `""`, with no parent walk and no GlobalString tail, unlike [`crate::ui_binder`]'s chain.
fn area_name(zone: u32, areas: Option<&AreaTableRes>) -> String {
    areas
        .and_then(|a| a.0.name(zone))
        .unwrap_or_default()
        .to_string()
}

/// Turn each Accept into `CMSG_SUMMON_RESPONSE`, gated on a latched summoner, never the deadline.
fn drain_summon(
    script: Option<NonSendMut<UiScript>>,
    summon: Res<SummonState>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    let confirms = script.take_summon_confirms();
    if confirms == 0 {
        return;
    }
    let Some(summoner) = summon.pending() else {
        debug!("ui_summon: ConfirmSummon() with nothing latched — nothing to answer with");
        return;
    };
    if benilla_assets::trace::enabled_for("summon") {
        benilla_assets::trace::line(
            "summon",
            &format!("SEND CMSG_SUMMON_RESPONSE summoner={summoner:#x} n={confirms}"),
        );
    }
    for _ in 0..confirms {
        let _ = commands.0.send(ClientCommand::SummonResponse { summoner });
    }
}

/// The latch dies with the session (the module's deviation), on `DisconnectedMessage`, which a
/// socket death, a kick and `/logout` all reach. A `/reload` keeps it, as the reference's does.
fn end_session_summon(
    mut msgs: MessageReader<crate::net::DisconnectedMessage>,
    mut summon: ResMut<SummonState>,
) {
    if msgs.read().next().is_some() {
        *summon = SummonState::default();
    }
}

/// The summon's packet handler.
pub(crate) mod net {
    use bevy::prelude::*;

    use super::SummonState;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;
    use crate::net::{GuidIndex, ObjectStore, SelfGuid};

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::SummonRequest, on_request);
    }

    /// `0x605f30` over the self object, not `unit_reads_dead` (`0x605f90`). An unstreamed self
    /// latches (`0x5e6189`).
    fn on_request(
        In(ev): In<SessionEvent>,
        mut summon: ResMut<SummonState>,
        self_guid: Res<SelfGuid>,
        index: Res<GuidIndex>,
        stores: Query<&ObjectStore>,
        real_clock: Res<Time<Real>>,
    ) {
        if let SessionEvent::SummonRequest {
            summoner,
            zone,
            delay_ms,
        } = ev
        {
            let dead_or_ghost = self_guid
                .0
                .and_then(|g| index.0.get(&g))
                .and_then(|e| stores.get(*e).ok())
                .is_some_and(|s| s.0.is_dead_or_ghost());
            request(
                summoner,
                zone,
                delay_ms,
                dead_or_ghost,
                real_clock.elapsed_secs_f64(),
                &mut summon,
            );
        }
    }

    /// `SMSG_SUMMON_REQUEST`: latch the offer unless `dead_or_ghost` (`0x605f30`), which touches
    /// nothing, so a live question survives.
    pub(crate) fn request(
        summoner: u64,
        zone: u32,
        delay_ms: u32,
        dead_or_ghost: bool,
        now: f64,
        summon: &mut SummonState,
    ) {
        // The `summon` trace's first link, with the gate's verdict: a refusal looks like a bug.
        if benilla_assets::trace::enabled_for("summon") {
            benilla_assets::trace::line(
                "summon",
                &format!(
                    "recv SMSG_SUMMON_REQUEST summoner={summoner:#x} zone={zone} \
                     delay_ms={delay_ms} dead_or_ghost={dead_or_ghost}"
                ),
            );
        }
        if dead_or_ghost {
            debug!("ui_summon: summon request from {summoner:#x} dropped — we are dead or a ghost");
            return;
        }
        summon.latch(summoner, zone, delay_ms, now);
    }
}

/// The summon flow: the question's feed and its one answer.
pub(crate) struct UiSummonPlugin;

impl Plugin for UiSummonPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<SummonState>().add_systems(
            Update,
            (
                // Before the feed, so the frame a session ends feeds nothing from it.
                end_session_summon.before(feed_summon),
                feed_summon.in_set(UiFeed),
                drain_summon.after(UiInput),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A decline leaves the guid latched, so only a per-packet flag sees the second summon.
    #[test]
    fn asking_twice_owes_two_dialogs() {
        let mut summon = SummonState::default();
        assert_eq!(summon.pending(), None);

        summon.latch(0x2a, 1519, 120_000, 100.0);
        assert!(summon.ask);
        summon.ask = false; // the feed fired the first dialog
        assert_eq!(summon.pending(), Some(0x2a), "the guid outlives the fire");

        summon.latch(0x2a, 1519, 120_000, 200.0);
        assert!(summon.ask, "the same summoner asking again owes a dialog");
    }

    /// `0x4963e0`'s three cases: never armed, running, expired.
    #[test]
    fn the_countdown_runs_from_the_packets_delay_and_never_goes_negative() {
        let mut summon = SummonState::default();
        assert_eq!(summon.time_left_ms(0.0), 0, "a zeroed deadline reads 0");

        summon.latch(0x2a, 1519, 120_000, 100.0);
        assert_eq!(summon.time_left_ms(100.0), 120_000);
        assert_eq!(summon.time_left_ms(160.0), 60_000);
        assert_eq!(summon.time_left_ms(220.0), 0, "expired, not negative");
        assert_eq!(summon.time_left_ms(9_999.0), 0);
    }

    /// A refused request leaves a live question exactly as it was.
    #[test]
    fn a_summon_that_arrives_while_refused_latches_nothing() {
        let mut summon = SummonState::default();
        net::request(0x2a, 1519, 120_000, true, 100.0, &mut summon);
        assert_eq!(summon.pending(), None);
        assert!(!summon.ask);
        assert_eq!(summon.time_left_ms(100.0), 0);

        // A live question, then a refused second request.
        net::request(0x2a, 1519, 120_000, false, 100.0, &mut summon);
        summon.ask = false;
        net::request(0x99, 1, 120_000, true, 150.0, &mut summon);
        assert_eq!(
            summon.pending(),
            Some(0x2a),
            "the live offer is undisturbed"
        );
        assert_eq!(summon.zone, 1519);
        assert!(!summon.ask, "and no second dialog is owed");
    }

    /// No parent-zone hop and no GlobalString tail, unlike [`crate::ui_binder`]'s chain.
    #[test]
    fn the_area_name_is_the_bare_row_with_no_fallback_chain() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let areas = AreaTableRes(
            benilla_formats::load_area_table_catalog(&mut chain).expect("AreaTable.dbc"),
        );

        assert_eq!(area_name(1519, Some(&areas)), "Stormwind City");
        // 186 is Dolanaar, a sub-area of Teldrassil: the row itself answers.
        assert_eq!(area_name(186, Some(&areas)), "Dolanaar");
        assert_eq!(area_name(0xffff, Some(&areas)), "", "no row, no name");
        assert_eq!(area_name(1519, None), "", "no catalog, no name");
    }

    /// The module's deviation: an Accept with nothing latched has nothing to send.
    #[test]
    fn nothing_pending_means_nothing_to_answer_with() {
        let summon = SummonState::default();
        assert_eq!(summon.pending(), None);
    }
}
