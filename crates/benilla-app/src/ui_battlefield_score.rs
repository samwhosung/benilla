//! The battleground scoreboard feed, the app half of the stock `WorldStateFrame.lua` score frame.
//!
//! The reference holds a `MSG_PVP_LOG_DATA` board until every row's name has resolved, and the last
//! name arrival rebuilds it (`0x4aa580` → `0x4aa200`); here the board is pushed the frame the last
//! name lands, and until then the VM's rebuild waits ([`UiScript::set_battlefield_score_names_pending`]).
//! `UPDATE_BATTLEFIELD_SCORE` fires on a pushed board and on the status-3 arrival, which resets
//! the filter and rebuilds unless names are pending, before `UPDATE_BATTLEFIELD_STATUS`
//! (`0x4aaa5a` before `0x4aab05`).

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_protocol::messages::PvpLogData;
use benilla_ui::script::{BattlefieldScoreRow, BattlefieldScores, BattlefieldStatColumn};

use crate::names::NameCache;
use crate::net::{NetCommands, WorldEnterCascadeMessage};
use crate::query_cache::Ask;
use crate::ui_dialog_verbs::BattlefieldQueue;
use crate::ui_script::UiFeed;
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

    /// The request throttle (`0x4aa170`): `true`, and the stamp moves, when a request may go out
    /// now.
    pub(crate) fn request_due(&mut self, now: Instant) -> bool {
        let throttled = self
            .last_request
            .is_some_and(|t| now.duration_since(t) < REQUEST_THROTTLE);
        if !throttled {
            self.last_request = Some(now);
        }
        !throttled
    }
}

/// The world-enter cascade's score reset (`0x4909fb` → `0x4a9db0`): the ended flag `0xb6ebcc`, the
/// winner `0xb6ebd0`, the wire and filtered counts `0xb6ebc0`/`0xb6ebc4`, the pending-name count
/// `0xb6ebd4` and the score request stamp `0xb6ebc8` all zero. The feed then clears the VM's
/// board. The cascade fires no event, and `SMSG_BATTLEFIELD_STATUS` is no part of it.
pub(crate) fn reset_on_world_enter(
    mut cascades: MessageReader<WorldEnterCascadeMessage>,
    mut board: ResMut<BattlefieldScoreboard>,
) {
    if cascades.read().next().is_some() {
        *board = BattlefieldScoreboard::default();
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

/// Resolve the raw board through the name cache; `None` while any name is still in flight. The
/// packet's handler looks each row's name up with the callback `0x4a9ef0` (`0x4aac79`), so `ask`
/// is the packet's [`Ask::Event`] on its first run.
fn resolve_board(
    log: &PvpLogData,
    names: &NameCache,
    ask: Ask<'_>,
    unit_names: crate::ui_unit::UnitNames<'_>,
) -> Option<Vec<BattlefieldScoreRow>> {
    let mut rows = Vec::with_capacity(log.rows.len());
    for r in &log.rows {
        let name = names.resolve_player(r.guid, ask)?.to_string();
        let (race, class) = names
            .player_traits(r.guid)
            .map_or((0, 0), |(race, class, _)| (race, class));
        // The team comes from race, never the wire (`0x4aa200`, `0x5efe00`'s walk): 0 Horde,
        // 1 Alliance, -1 neither.
        let faction = i32::from(unit_names.pvp_team(race));
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
            race: unit_names.race(race).map(|(n, _)| n.to_string()),
            class: unit_names.class(class).map(|(n, _)| n.to_string()),
            stats,
        });
    }
    Some(rows)
}

#[allow(clippy::type_complexity)] // a Bevy system's full input set
pub(crate) fn feed_battlefield_score(
    // The board and the queue, which the score and leave calls read and stamp: an engine turn,
    // its events signalled after.
    mut vm: crate::script_calls::VmWith<(
        Res<'static, BattlefieldScoreboard>,
        ResMut<'static, BattlefieldQueue>,
    )>,
    catalog: Option<Res<WorldStateUiRes>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    unit_names: crate::ui_unit::NameTables,
    mut last: Local<crate::ui_script::VmMemo<Option<BattlefieldScores>>>,
) {
    vm.turn(|script, (board, mut queue)| {
        let Some(script) = script else {
            return;
        };
        let now = Instant::now();
        script.set_battlefield_run_time_ms(queue.run_time_ms(now));

        let ask = Ask::at(board.is_changed(), &commands);
        let fresh = board.log.as_ref().and_then(|log| {
            let rows = resolve_board(log, &names, ask, unit_names.get())?;
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
        let last = last.get(script);
        // The cascade's reset (`0x4a9db0`) leaves no board: the VM's is cleared too, with the
        // filter back to every team and no event, as the reference fires none.
        if board.log.is_none() && last.take().is_some() {
            script.clear_battlefield_scores();
        }
        // A held-back board is the reference's pending names: the VM's rebuild waits on it.
        script.set_battlefield_score_names_pending(board.log.is_some() && fresh.is_none());
        if queue.take_score_dirty() {
            script.reset_battlefield_score_filter();
        }
        if fresh.is_some() && fresh != *last {
            script.set_battlefield_scores(fresh.clone().expect("checked"));
            *last = fresh;
            script.fire_event("UPDATE_BATTLEFIELD_SCORE", vec![]);
        }
    })
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
                reset_on_world_enter
                    .before(feed_battlefield_score)
                    .in_set(UiFeed),
                // Before the queue feed: on status 3 the score event precedes the status event.
                feed_battlefield_score
                    .before(crate::ui_dialog_verbs::feed_dialog_verbs)
                    .in_set(UiFeed),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::{WorldStateUiCatalog, WorldStateUiRow};
    use benilla_ui::script::UiScript;

    /// The log packet looks each row's name up with the callback `0x4a9ef0` (`0x4aac79`); the
    /// frames its board waits through read. A refused name is asked by no frame after it, only by
    /// the next log.
    #[test]
    fn a_refused_score_row_is_asked_by_the_next_log_never_by_a_frame() {
        use benilla_protocol::messages::PvpLogRow;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<BattlefieldScoreboard>()
            .init_resource::<BattlefieldQueue>()
            .init_resource::<NameCache>()
            .add_systems(Update, feed_battlefield_score);
        app.insert_non_send_resource(UiScript::new().unwrap());
        let log = |app: &mut App| {
            app.world_mut()
                .resource_mut::<BattlefieldScoreboard>()
                .apply(PvpLogData {
                    ended: false,
                    winner: None,
                    rows: vec![PvpLogRow {
                        guid: 0x9,
                        rank: 0,
                        killing_blows: 0,
                        honorable_kills: 0,
                        deaths: 0,
                        honor_gained: 0,
                        stats: Vec::new(),
                    }],
                });
            app.update();
        };
        log(&mut app);
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(0x9, String::new(), None);
        for _ in 0..600 {
            app.update();
        }
        assert!(
            crate::query_cache::queries(&rx).is_empty(),
            "no frame asks a refusal"
        );
        log(&mut app);
        assert_eq!(
            crate::query_cache::queries(&rx).len(),
            1,
            "the next log asks again"
        );
    }

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

    /// The world-enter cascade (`0x4909fb` → `0x4a9db0`) zeroes the ended flag `0xb6ebcc`, the
    /// winner `0xb6ebd0`, the counts `0xb6ebc0`/`0xb6ebc4` and the score request stamp `0xb6ebc8`:
    /// the stock end-of-match panel (`WorldStateFrame.lua:192`) reads `GetBattlefieldWinner()`.
    #[test]
    fn a_world_enter_clears_the_finished_board_and_its_request_stamp() {
        use benilla_protocol::messages::PvpLogRow;
        let mut app = App::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx))
            .add_message::<crate::net::WorldEnterCascadeMessage>()
            .init_resource::<BattlefieldScoreboard>()
            .init_resource::<BattlefieldQueue>()
            .init_resource::<NameCache>()
            .add_systems(
                Update,
                (reset_on_world_enter, feed_battlefield_score).chain(),
            );
        app.insert_non_send_resource(UiScript::new().unwrap());
        let guid = 0x9;
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(guid, "Rik".into(), None);
        let now = Instant::now();
        {
            let mut board = app.world_mut().resource_mut::<BattlefieldScoreboard>();
            board.apply(PvpLogData {
                ended: true,
                winner: Some(1),
                rows: vec![PvpLogRow {
                    guid,
                    rank: 0,
                    killing_blows: 3,
                    honorable_kills: 0,
                    deaths: 0,
                    honor_gained: 0,
                    stats: Vec::new(),
                }],
            });
            assert!(board.request_due(now));
        }
        app.update();
        let eval = |app: &App, lua: &str| -> i64 {
            app.world()
                .non_send_resource::<UiScript>()
                .eval(lua)
                .unwrap()
        };
        assert_eq!(eval(&app, "return GetBattlefieldWinner()"), 1);
        assert_eq!(eval(&app, "return GetNumBattlefieldScores()"), 1);

        app.world_mut()
            .write_message(crate::net::WorldEnterCascadeMessage);
        app.update();

        assert!(app
            .world()
            .non_send_resource::<UiScript>()
            .eval::<bool>("return GetBattlefieldWinner() == nil")
            .unwrap());
        assert_eq!(eval(&app, "return GetNumBattlefieldScores()"), 0);
        let mut board = app.world_mut().resource_mut::<BattlefieldScoreboard>();
        assert!(board.log.is_none());
        assert!(
            board.request_due(now),
            "the stamp is zeroed, so the next request is not throttled"
        );
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
