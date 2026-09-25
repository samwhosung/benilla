//! The battleground scoreboard feed, the app half of the stock `WorldStateFrame.lua` score frame.
//!
//! The reference holds a `MSG_PVP_LOG_DATA` board until every row's name has resolved, and the last
//! name arrival rebuilds it (`0x4aa580` → `0x4aa200`); here the board is pushed the frame the last
//! name lands. `UPDATE_BATTLEFIELD_SCORE` fires on a pushed board and on the status-3 arrival,
//! before `UPDATE_BATTLEFIELD_STATUS` (`0x4aaa5a` before `0x4aab05`).

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_protocol::messages::PvpLogData;
use benilla_ui::script::{BattlefieldScoreRow, BattlefieldScores, BattlefieldStatColumn, UiScript};

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_dialog_verbs::BattlefieldQueue;
use crate::ui_script::{UiFeed, UiInput};
use crate::world_state_ui::WorldStateUiRes;

/// `RequestBattlefieldScoreData`'s throttle (`0x4aa170`).
const REQUEST_THROTTLE: Duration = Duration::from_millis(5000);

/// The last `MSG_PVP_LOG_DATA` and the request stamp, both zeroed when the session ends.
#[derive(Resource, Default)]
pub(crate) struct BattlefieldScoreboard {
    log: Option<PvpLogData>,
    last_request: Option<Instant>,
}

impl BattlefieldScoreboard {
    pub(crate) fn apply(&mut self, data: PvpLogData) {
        self.log = Some(data);
    }
}

/// The column headers for `map`: the first contiguous run of matching `WorldStateUI.dbc` rows in
/// table order, as the status-3 arm builds them (`0x4aa9c3`–`0x4aaa17`; the run ends at
/// `0x4aa9fe`).
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
        // The team comes from race, never the wire (`0x4aa200`, `0x5efe00`'s walk): 0 Horde,
        // 1 Alliance, -1 neither.
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

/// The scoreboard's packet handlers.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::BattlefieldScoreboard;
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::PvpLogData, on_pvp_log_data)
            .net_handler(SessionEventKind::Disconnected, on_session_end);
    }

    /// Zeroes the board and the stamp, as the reference's module init (`0x4a9c40`) does at every
    /// login.
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
                // Before the queue feed: on status 3 the score event precedes the status event.
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
        // Another map gets the wildcard row alone: `-1` rows serve every map.
        let cols: Vec<String> = score_columns(&cat, 30)
            .into_iter()
            .map(|c| c.text)
            .collect();
        assert_eq!(cols, ["Flags Returned"]);
    }

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
