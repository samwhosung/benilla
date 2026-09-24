//! The battleground **scoreboard** feed (decision 1972):
//! the app's half of the stock `WorldStateFrame.lua` score frame — the name-resolution barrier,
//! the team derivation, the column headers, the request throttle and the leave.
//!
//! - **The board arrives raw** (`MSG_PVP_LOG_DATA`, GUIDs and numbers, wire order) and the
//!   reference does nothing with it until EVERY row's name has resolved (`0x4aa580` → `0x4aa200`:
//!   the last name arrival is what rebuilds and fires). Ours asks the name cache for each row every
//!   frame it is unresolved and pushes the board the frame the last one lands.
//! - **Team is derived, never wire data** (`0x4aa200`): race → faction; `0` Horde, `1` Alliance,
//!   `-1` for a race the tables do not carry. Race and class strings come off the same traits the
//!   name query answered with.
//! - **Columns are `WorldStateUI.dbc` rows** (the `0x2D4` status-3 arm, `0x4aa9c3`–`0x4aaa17`):
//!   in table order, the first contiguous run of rows whose `MapID` is the battleground's map or
//!   `-1` and whose `Type` is 2; the text raw, the icon, the tooltip.
//! - **`UPDATE_BATTLEFIELD_SCORE`** fires here — on a pushed board, and on the status-3 arrival
//!   the queue flags — BEFORE the queue feed fires `UPDATE_BATTLEFIELD_STATUS` (`0x4aaa5a` before
//!   `0x4aab05`).
//! - **The request is throttled to 5000 ms** (`0x4aa170`) and the leave carries the active map
//!   (`LeaveBattlefield 0x4abe60`).

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_protocol::messages::PvpLogData;
use benilla_ui::script::{BattlefieldScoreRow, BattlefieldScores, BattlefieldStatColumn, UiScript};

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_dialog_verbs::BattlefieldQueue;
use crate::ui_script::{UiFeed, UiInput};
use crate::world_state_ui::WorldStateUiRes;

/// `RequestBattlefieldScoreData`'s throttle — `0x4aa170`: `now + 0x1388`.
const REQUEST_THROTTLE: Duration = Duration::from_millis(5000);

/// The last `MSG_PVP_LOG_DATA`, and the request stamp. Both die with the session
/// (`net::on_session_end`).
#[derive(Resource, Default)]
pub(crate) struct BattlefieldScoreboard {
    log: Option<PvpLogData>,
    last_request: Option<Instant>,
}

impl BattlefieldScoreboard {
    /// `SessionEvent::PvpLogData` — the whole board, replacing the last.
    pub(crate) fn apply(&mut self, data: PvpLogData) {
        self.log = Some(data);
    }
}

/// The scoreboard's column headers for `map`: `WorldStateUI.dbc` in table order, the first
/// contiguous run of `(MapID == map || MapID == -1) && Type == 2` — once a run has started, the
/// first non-matching row ends it (`0x4aa9fe`).
pub(crate) fn score_columns(catalog: &WorldStateUiRes, map: u32) -> Vec<BattlefieldStatColumn> {
    let mut out = Vec::new();
    for (_, row) in catalog.0.rows() {
        let matches = (row.map_id == map || row.map_id == u32::MAX) && row.ui_type == 2;
        if matches {
            out.push(BattlefieldStatColumn {
                text: row.text.clone(),
                icon: row.icon.clone(),
                tooltip: row.tooltip.clone(),
            });
        } else if !out.is_empty() {
            break;
        }
    }
    out
}

/// Resolve the raw board through the name cache; `None` while any name is still in flight.
fn resolve_board(
    log: &PvpLogData,
    names: &NameCache,
    commands: &NetCommands,
) -> Option<Vec<BattlefieldScoreRow>> {
    let mut rows = Vec::with_capacity(log.rows.len());
    for r in &log.rows {
        let name = names.resolve(r.guid, commands)?.to_string();
        let (race, class) = names
            .player_traits(r.guid)
            .map_or((0, 0), |(race, class, _)| (race, class));
        // `0` Horde, `1` Alliance, `-1` neither. `0x4aa200` walks the name-cache record's race
        // through `ChrRaces` → `FactionTemplate` → factionGroupMask, which is `0x5efe00`'s walk
        // with the null-row guards omitted — so it is [`crate::ui_unit::race_pvp_team`] and not a
        // second copy of it. (It used to be one, re-derived here out of `race_faction_group`; a
        // second copy of this walk is exactly what report B378 was, one surface over.)
        let faction = i32::from(crate::ui_unit::race_pvp_team(race));
        let mut stats = [0u32; 8];
        for (slot, v) in stats.iter_mut().zip(&r.stats) {
            *slot = *v;
        }
        rows.push(BattlefieldScoreRow {
            name,
            killing_blows: r.killing_blows,
            honorable_kills: r.honorable_kills,
            deaths: r.deaths,
            honor_gained: r.honor_gained,
            faction,
            rank: i32::try_from(r.rank).unwrap_or(i32::MAX),
            race: crate::ui_unit::race_names(race).map(|(n, _)| n.to_string()),
            class: crate::ui_unit::class_names(class).map(|(n, _)| n.to_string()),
            stats,
        });
    }
    Some(rows)
}

pub(crate) fn feed_battlefield_score(
    script: Option<NonSendMut<UiScript>>,
    board: Res<BattlefieldScoreboard>,
    mut queue: ResMut<BattlefieldQueue>,
    catalog: Option<Res<WorldStateUiRes>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut last: Local<crate::ui_script::VmMemo<Option<BattlefieldScores>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let now = Instant::now();
    script.set_battlefield_run_time_ms(queue.run_time_ms(now));

    let fresh = board.log.as_ref().and_then(|log| {
        let rows = resolve_board(log, &names, &commands)?;
        let columns = queue
            .active_map()
            .zip(catalog.as_deref())
            .map(|(map, cat)| score_columns(cat, map))
            .unwrap_or_default();
        Some(BattlefieldScores {
            rows,
            ended: log.ended,
            winner: log.winner.unwrap_or(0),
            columns,
        })
    });
    let last = last.get(&script);
    let status_rebuild = queue.take_score_dirty();
    let changed = fresh.is_some() && fresh != *last;
    if changed {
        script.set_battlefield_scores(fresh.clone().expect("checked"));
        *last = fresh;
    }
    if changed || status_rebuild {
        script.fire_event("UPDATE_BATTLEFIELD_SCORE", vec![]);
    }
}

fn drain_battlefield_score(
    script: Option<NonSendMut<UiScript>>,
    mut board: ResMut<BattlefieldScoreboard>,
    queue: Res<BattlefieldQueue>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    if script.take_battlefield_score_requests() > 0 {
        let now = Instant::now();
        let throttled = board
            .last_request
            .is_some_and(|t| now.duration_since(t) < REQUEST_THROTTLE);
        if !throttled {
            board.last_request = Some(now);
            let _ = commands.0.send(ClientCommand::RequestBattlefieldScoreData);
        }
    }
    if script.take_battlefield_leave_requests() > 0 {
        let _ = commands.0.send(ClientCommand::LeaveBattlefield {
            map_id: queue.active_map().unwrap_or(0),
        });
    }
}

/// The scoreboard's packet handler (in the net handler table since 2313).
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::BattlefieldScoreboard;
    use crate::net::NetHandlerApp;

    /// Register the handler — called from [`super::BattlefieldScorePlugin`].
    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::PvpLogData, on_pvp_log_data)
            .net_handler(SessionEventKind::Disconnected, on_session_end);
    }

    /// The board and the request stamp are zeroed at every login (module init `0x4a9c40` zeroes
    /// the score scalars) — a listener on the session end (a second handler on the kind, after the
    /// bridge's own teardown).
    fn on_session_end(In(_): In<SessionEvent>, mut board: ResMut<BattlefieldScoreboard>) {
        *board = BattlefieldScoreboard::default();
    }

    fn on_pvp_log_data(In(ev): In<SessionEvent>, mut board: ResMut<BattlefieldScoreboard>) {
        if let SessionEvent::PvpLogData(data) = ev {
            board.apply(data);
        }
    }
}

pub(crate) struct BattlefieldScorePlugin;

impl Plugin for BattlefieldScorePlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<BattlefieldScoreboard>().add_systems(
            Update,
            (
                // Before the queue feed: the score event precedes the status event on the
                // status-3 message, as in the client.
                feed_battlefield_score
                    .before(crate::ui_dialog_verbs::feed_dialog_verbs)
                    .in_set(UiFeed),
                drain_battlefield_score.after(UiInput),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::{WorldStateUiCatalog, WorldStateUiRow};

    fn row(map_id: u32, ui_type: u32, text: &str) -> WorldStateUiRow {
        WorldStateUiRow {
            map_id,
            area_id: 0,
            icon: String::new(),
            text: text.into(),
            tooltip: String::new(),
            state_variable: 0,
            ui_type,
            dynamic_icon: String::new(),
            dynamic_tooltip: String::new(),
            extended_ui: String::new(),
            extended_ui_state: [0; 3],
        }
    }

    /// The column scan: table order, the map's or the wildcard's `Type == 2` rows, and the first
    /// non-matching row after the run has started ends it.
    #[test]
    fn the_columns_are_the_first_contiguous_run_of_type_two_rows_for_the_map() {
        let cat = WorldStateUiRes(WorldStateUiCatalog::from_rows(vec![
            (1, row(489, 0, "always-up")),
            (2, row(529, 2, "other map")),
            (3, row(489, 2, "Flags Captured")),
            (4, row(u32::MAX, 2, "Flags Returned")),
            (5, row(489, 0, "a gap")),
            (6, row(489, 2, "after the gap")),
        ]));
        let cols: Vec<String> = score_columns(&cat, 489)
            .into_iter()
            .map(|c| c.text)
            .collect();
        assert_eq!(cols, ["Flags Captured", "Flags Returned"]);
        // Another map keeps the wildcard row alone — the reference's `-1` rows serve every map.
        let cols: Vec<String> = score_columns(&cat, 30)
            .into_iter()
            .map(|c| c.text)
            .collect();
        assert_eq!(cols, ["Flags Returned"]);
    }

    /// **The board is zeroed at every login** (module
    /// init `0x4a9c40`, from `InitializeGame`, zeroes the score scalars) — the last session's
    /// `MSG_PVP_LOG_DATA` must not come back as the next character's scoreboard, and its request
    /// stamp must not throttle that character's first ask.
    #[test]
    fn the_session_end_zeroes_the_scoreboard() {
        let mut app = App::new();
        app.init_resource::<BattlefieldScoreboard>();
        net::register(&mut app);
        {
            let mut board = app.world_mut().resource_mut::<BattlefieldScoreboard>();
            board.apply(PvpLogData {
                ended: true,
                winner: Some(1),
                rows: Vec::new(),
            });
            board.last_request = Some(Instant::now());
        }

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );

        let board = app.world().resource::<BattlefieldScoreboard>();
        assert!(board.log.is_none());
        assert!(board.last_request.is_none());
    }
}
