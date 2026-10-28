//! The battleground scoreboard, the app half of the stock `WorldStateFrame.lua` score frame: the
//! board the reference keeps (`0xb6e880`), its filter, its sort and its stat columns, which the
//! getters read through the lent host at the call ([`crate::ui_battlefield::BattlefieldLive`]).
//!
//! `MSG_PVP_LOG_DATA` (`0x4aab30`) stores its rows in wire order and asks each row's name with the
//! callback `0x4a9ef0`, a miss counting one more name pending (`0x4aac82`); with none pending it
//! rebuilds at once (`0x4aacac`), else the last answer rebuilds (`0x4aa580`). The rebuild
//! (`0x4aa200`) sets each row's team, counts the filter's rows, sorts and signals
//! `UPDATE_BATTLEFIELD_SCORE` (`0x4aa308`) on the battleground lane, at the packet whose handler ran
//! it; the status-3 arrival's filter reset rebuilds the same way, before its
//! `UPDATE_BATTLEFIELD_STATUS` (`0x4aaa5a` before `0x4aab05`).

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_protocol::messages::PvpLogData;
use benilla_ui::script::{BattlefieldScoreRead, BattlefieldScoreRow, BattlefieldStatColumn};

use crate::names::NameCache;
use crate::net::NetCommands;
use crate::query_cache::Ask;
use crate::world_state_ui::WorldStateUiRes;

/// `RequestBattlefieldScoreData`'s throttle (`0x4aa170`).
const REQUEST_THROTTLE: Duration = Duration::from_millis(5000);

/// One score block (`0x40` bytes): the wire's row and the team the last rebuild set (`+0x18`).
#[derive(Clone, Debug, PartialEq, Eq)]
struct ScoreBlock {
    guid: u64,
    killing_blows: u32,
    honorable_kills: u32,
    deaths: u32,
    honor_gained: u32,
    team: i8,
    rank: u32,
    stats: [u32; 8],
}

/// The board, its filter and the request stamp, all zeroed when the session ends, and at each
/// world-enter cascade by its battlefield init (`0x4909fb` → `0x4a9db0`,
/// [`crate::ui_battlefield::reset_on_world_enter`]), which fires no event.
#[derive(Resource)]
pub(crate) struct BattlefieldScoreboard {
    /// The blocks in board order (`0xb6e880`): the last message's in wire order, sorted in place
    /// by each rebuild. Their count is the wire count (`0xb6ebc0`).
    rows: Vec<ScoreBlock>,
    /// The filtered count (`0xb6ebc4`), written by the rebuild alone.
    filtered: usize,
    /// The ended flag (`0xb6ebcc`) and the winner (`0xb6ebd0`).
    ended: bool,
    winner: u8,
    /// The faction filter (`0x8457d0`): -1 every team, 0 Horde, 1 Alliance.
    filter: i8,
    /// The stat columns the status-3 arm built (`0xb6e6ec`, count `0xb6ebd8`).
    columns: Vec<BattlefieldStatColumn>,
    /// The guids whose name answer a callback `0x4a9ef0` waits on, one per miss: their count is
    /// the pending-name count (`0xb6ebd4`).
    name_waits: Vec<u64>,
    last_request: Option<Instant>,
}

impl Default for BattlefieldScoreboard {
    /// The filter starts at -1 (`0x4a9c40`).
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            filtered: 0,
            ended: false,
            winner: 0,
            filter: -1,
            columns: Vec::new(),
            name_waits: Vec::new(),
            last_request: None,
        }
    }
}

/// What the board's rebuild reads: the name cache's records and the race tables.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct BoardReads<'w> {
    names: Option<Res<'w, NameCache>>,
    tables: crate::ui_unit::NameTables<'w>,
}

impl BoardReads<'_> {
    fn view(&self) -> Option<BoardView<'_>> {
        Some(BoardView {
            names: self.names.as_deref()?,
            tables: self.tables.get(),
        })
    }
}

/// [`BoardReads`] borrowed, or what a handler holding the name cache itself lends.
#[derive(Clone, Copy)]
pub(crate) struct BoardView<'a> {
    pub(crate) names: &'a NameCache,
    pub(crate) tables: crate::ui_unit::UnitNames<'a>,
}

impl BattlefieldScoreboard {
    /// `MSG_PVP_LOG_DATA` (`0x4aab30`): the flags and the blocks stored, each row's name asked
    /// with a callback (`0x4aac79`), a miss pending (`0x4aac82`); then the rebuild unless a name
    /// is pending (`0x4aaca1`). Whether it rebuilt, when the caller signals.
    pub(crate) fn apply(
        &mut self,
        data: PvpLogData,
        commands: &NetCommands,
        at: BoardView,
    ) -> bool {
        self.ended = data.ended;
        self.winner = data.winner.unwrap_or(0);
        self.rows = data
            .rows
            .into_iter()
            .map(|r| {
                let mut stats = [0u32; 8];
                for (slot, v) in stats.iter_mut().zip(&r.stats) {
                    *slot = *v;
                }
                ScoreBlock {
                    guid: r.guid,
                    killing_blows: r.killing_blows,
                    honorable_kills: r.honorable_kills,
                    deaths: r.deaths,
                    honor_gained: r.honor_gained,
                    team: -1,
                    rank: r.rank,
                    stats,
                }
            })
            .collect();
        for i in 0..self.rows.len() {
            let guid = self.rows[i].guid;
            if at
                .names
                .resolve_player(guid, Ask::Event(commands))
                .is_none()
            {
                self.name_waits.push(guid);
            }
        }
        self.rebuild(at)
    }

    /// A name answer, a record or a refusal: each callback waiting on it counts down
    /// (`0x4a9ef0` → `0x4aa580`), and the last rebuilds. Whether it rebuilt.
    pub(crate) fn name_answered(&mut self, guid: u64, at: BoardView) -> bool {
        let before = self.name_waits.len();
        self.name_waits.retain(|&g| g != guid);
        before != self.name_waits.len() && self.rebuild(at)
    }

    /// `0x4aa5a0(f)`: `f` stored, then the rebuild. Whether it rebuilt.
    pub(crate) fn set_filter(&mut self, filter: i8, reads: &BoardReads) -> bool {
        self.filter = filter;
        reads.view().is_some_and(|at| self.rebuild(at))
    }

    /// The status-3 arm (`0x4aa9c3`–`0x4aaa5a`): the stat columns for `map` built, then
    /// `0x4aa5a0(-1)`. Whether it rebuilt.
    pub(crate) fn status_three(
        &mut self,
        map: u32,
        catalog: Option<&WorldStateUiRes>,
        reads: &BoardReads,
    ) -> bool {
        self.columns = catalog.map_or_else(Vec::new, |c| score_columns(c, map));
        self.set_filter(-1, reads)
    }

    /// A zero-map status's clear of the stat count (`0xb6ebd8`).
    pub(crate) fn clear_columns(&mut self) {
        self.columns.clear();
    }

    /// `0x4aa200`: nothing while a name is pending (`0x4aa216`); else each row's team from its
    /// record's race (`0x5efe00`'s walk inlined), 0 for a row with no record (`0x4aa26d` →
    /// `0x4aa2b5`), the filter's count, and the CRT `qsort` under `0x4aa350` (`0x4aa2ee`-`0x4aa2fb`).
    fn rebuild(&mut self, at: BoardView) -> bool {
        if !self.name_waits.is_empty() {
            return false;
        }
        // A record's side by its race; the cache holds a record's race beside its name.
        let side = |guid: u64| {
            at.names
                .player_traits(guid)
                .map(|(race, _, _)| at.tables.pvp_team(race))
        };
        for row in &mut self.rows {
            row.team = side(row.guid).unwrap_or(0);
        }
        let filter = self.filter;
        self.filtered = if filter == -1 {
            self.rows.len()
        } else {
            self.rows.iter().filter(|r| r.team == filter).count()
        };
        let name = |guid: u64| at.names.resolve_player(guid, Ask::Read);
        benilla_ui::crt::qsort(&mut self.rows, |a, b| {
            // Either record missing: 0 (`0x4aa3c9`, `0x4aa3d1`).
            let (Some(na), Some(nb)) = (name(a.guid), name(b.guid)) else {
                return 0;
            };
            // Sides apart: the filter's first, +1 both ways when neither is it, read off the
            // records, not the blocks (`0x4aa3e4`-`0x4aa4fa`).
            if filter != -1 {
                let (ta, tb) = (side(a.guid).unwrap_or(-1), side(b.guid).unwrap_or(-1));
                if ta != tb {
                    return if ta != filter { 1 } else { -1 };
                }
            }
            // Kills down, deaths up, honor down (`0x4aa512`-`0x4aa54e`), then `SStrCmpI`
            // (`0x4aa567`).
            let down = |x: u32, y: u32| (x != y).then_some(if y < x { -1 } else { 1 });
            down(a.killing_blows, b.killing_blows)
                .or_else(|| down(b.deaths, a.deaths))
                .or_else(|| down(a.honor_gained, b.honor_gained))
                .unwrap_or_else(|| benilla_ui::crt::str_cmp_i(na, nb))
        });
        true
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

    /// The scalars `GetNumBattlefieldScores`, `GetBattlefieldWinner`, `LeaveBattlefield` and
    /// `GetNumBattlefieldStats` read.
    pub(crate) fn summary(&self) -> benilla_ui::script::BattlefieldScoreSummary {
        benilla_ui::script::BattlefieldScoreSummary {
            filtered: self.filtered,
            ended: self.ended,
            winner: self.winner,
            columns: self.columns.len(),
        }
    }

    /// Row `index` (1-based) in board order, bounded by the wire count, read with its record as
    /// it stands (`0x4ab9d0`): the name, the race and class names (`0x4aba6d`).
    pub(crate) fn row(&self, index: i32, at: BoardView) -> Option<BattlefieldScoreRead> {
        let block = self
            .rows
            .get(usize::try_from(index).ok()?.checked_sub(1)?)?;
        let name = at.names.resolve_player(block.guid, Ask::Read);
        let (race, class) = at
            .names
            .player_traits(block.guid)
            .map_or((0, 0), |(race, class, _)| (race, class));
        Some(BattlefieldScoreRead {
            named: name.is_some(),
            row: BattlefieldScoreRow {
                name: name.unwrap_or_default().to_string(),
                killing_blows: block.killing_blows,
                honorable_kills: block.honorable_kills,
                deaths: block.deaths,
                honor_gained: block.honor_gained,
                faction: i32::from(block.team),
                // `fild dword` (`0x4abb47`): the word read signed.
                rank: block.rank as i32,
                race: at.tables.race(race).map(|(n, _)| n.to_string()),
                class: at.tables.class(class).map(|(n, _)| n.to_string()),
                stats: block.stats,
            },
        })
    }

    /// Stat column `index` (1-based), `None` past the count (`0x4abd41`).
    pub(crate) fn column(&self, index: i32) -> Option<BattlefieldStatColumn> {
        self.columns
            .get(usize::try_from(index).ok()?.checked_sub(1)?)
            .cloned()
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

/// The scoreboard's packet handlers.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{BattlefieldScoreboard, BoardReads};
    use crate::net::{NetCommands, NetHandlerApp};
    use crate::ui_battlefield::{BattlefieldEvent, BattlefieldLane};

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::PvpLogData, on_pvp_log_data)
            .net_handler(SessionEventKind::Disconnected, on_session_end);
    }

    /// Zeroes the board and the stamp, as the reference's module init (`0x4a9c40`) does at every
    /// login.
    fn on_session_end(In(_): In<SessionEvent>, mut board: ResMut<BattlefieldScoreboard>) {
        *board = BattlefieldScoreboard::default();
    }

    fn on_pvp_log_data(
        In(ev): In<SessionEvent>,
        mut board: ResMut<BattlefieldScoreboard>,
        mut lane: ResMut<BattlefieldLane>,
        reads: BoardReads,
        commands: Res<NetCommands>,
    ) {
        let SessionEvent::PvpLogData(data) = ev else {
            return;
        };
        let Some(at) = reads.view() else {
            return;
        };
        if board.apply(data, &commands, at) {
            lane.push(BattlefieldEvent::Score);
        }
    }
}

pub(crate) struct BattlefieldScorePlugin;

impl Plugin for BattlefieldScorePlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<BattlefieldScoreboard>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::{WorldStateUiCatalog, WorldStateUiRow};
    use benilla_protocol::messages::PvpLogRow;
    use benilla_protocol::SessionEvent;

    use crate::ui_battlefield::{BattlefieldEvent, BattlefieldLane};

    /// The log's and the name answer's handlers, in a world of their own with the stock race
    /// tables: race 1 is the Alliance's, race 2 the Horde's.
    fn board() -> (App, crossbeam_channel::Receiver<crate::net::ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<BattlefieldScoreboard>()
            .init_resource::<BattlefieldLane>()
            .init_resource::<NameCache>();
        crate::ui_unit::TestNames::english().insert(&mut app);
        net::register(&mut app);
        crate::names::net::register(&mut app);
        (app, rx)
    }

    fn packet(app: &mut App, ev: SessionEvent) -> Vec<BattlefieldEvent> {
        crate::net::handlers::dispatch(app.world_mut(), vec![ev]);
        app.world_mut().resource_mut::<BattlefieldLane>().take()
    }

    fn score(guid: u64, kills: u32, deaths: u32, honor: u32) -> PvpLogRow {
        PvpLogRow {
            guid,
            rank: 3,
            killing_blows: kills,
            honorable_kills: 0,
            deaths,
            honor_gained: honor,
            stats: vec![1, 2],
        }
    }

    fn log(ended: bool, rows: Vec<PvpLogRow>) -> SessionEvent {
        SessionEvent::PvpLogData(PvpLogData {
            ended,
            winner: ended.then_some(1),
            rows,
        })
    }

    fn named(guid: u64, name: &str, race: u32) -> SessionEvent {
        SessionEvent::PlayerName {
            guid,
            name: name.into(),
            race,
            gender: 0,
            class: 1,
        }
    }

    /// The board's names in board order, and the filtered count.
    fn names(app: &App) -> (Vec<String>, usize) {
        let world = app.world();
        let board = world.resource::<BattlefieldScoreboard>();
        let at = BoardView {
            names: world.resource::<NameCache>(),
            tables: crate::ui_unit::world_unit_names(world),
        };
        let rows = (1..=board.rows.len() as i32)
            .map(|i| board.row(i, at).map_or_else(String::new, |r| r.row.name))
            .collect();
        (rows, board.filtered)
    }

    /// The log asks each row's name with the callback `0x4a9ef0` (`0x4aac79`), a miss pending;
    /// the rebuild and its `UPDATE_BATTLEFIELD_SCORE` wait for the last answer, a refusal
    /// counting as one (`0x4aa580`), and then sort the board as `0x4aa350` does: kills down,
    /// deaths up, honor down, the name without case, a row with no record tied with all. A held
    /// board rebuilds inside its packet.
    #[test]
    fn the_rebuild_waits_for_the_last_name_and_sorts_like_the_client() {
        let (mut app, rx) = board();
        for (guid, name, race) in [(0x1, "bob", 2), (0x2, "Alice", 1)] {
            packet(&mut app, named(guid, name, race));
        }
        let rows = vec![
            score(0x1, 3, 1, 50),
            score(0x2, 5, 2, 10),
            score(0x3, 5, 1, 10),
            score(0x4, 5, 1, 20),
        ];
        assert_eq!(
            packet(&mut app, log(false, rows.clone())),
            [],
            "two names pending"
        );
        assert_eq!(crate::query_cache::queries(&rx).len(), 2);
        assert_eq!(packet(&mut app, named(0x3, "Carl", 1)), []);
        assert_eq!(
            packet(&mut app, named(0x4, "", 0)),
            [BattlefieldEvent::Score],
            "the refusal is the last answer"
        );
        assert_eq!(
            names(&app),
            (
                vec!["Carl".into(), "Alice".into(), String::new(), "bob".into()],
                4
            ),
            "the refused row ties every row, and stays where qsort's shortsort leaves it"
        );
        packet(&mut app, named(0x4, "Dana", 2));
        assert_eq!(
            packet(&mut app, log(false, rows)),
            [BattlefieldEvent::Score]
        );
        assert_eq!(
            names(&app),
            (
                vec!["Dana".into(), "Carl".into(), "Alice".into(), "bob".into()],
                4
            )
        );
        let mut reads = bevy::ecs::system::SystemState::<BoardReads>::new(app.world_mut());
        let world = app.world_mut();
        let rebuilt = world.resource_scope(|world, mut board: Mut<BattlefieldScoreboard>| {
            board.set_filter(1, &reads.get(world))
        });
        assert!(rebuilt);
        assert_eq!(
            names(&app),
            (
                vec!["Carl".into(), "Alice".into(), "Dana".into(), "bob".into()],
                2
            ),
            "the filter's team first, the rest still reachable"
        );
    }

    /// A refused name is asked by no frame after the log, only by the next log.
    #[test]
    fn a_refused_score_row_is_asked_by_the_next_log_never_by_a_frame() {
        let (mut app, rx) = board();
        packet(&mut app, log(false, vec![score(0x9, 0, 0, 0)]));
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
        packet(&mut app, named(0x9, "", 0));
        for _ in 0..600 {
            app.update();
        }
        assert!(
            crate::query_cache::queries(&rx).is_empty(),
            "no frame asks a refusal"
        );
        packet(&mut app, log(false, vec![score(0x9, 0, 0, 0)]));
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
    /// winner `0xb6ebd0`, the counts `0xb6ebc0`/`0xb6ebc4`, the pending names `0xb6ebd4` and the
    /// score request stamp `0xb6ebc8`: the stock end-of-match panel (`WorldStateFrame.lua:192`)
    /// reads `GetBattlefieldWinner()`.
    #[test]
    fn a_world_enter_clears_the_finished_board_and_its_request_stamp() {
        let (mut app, _rx) = board();
        packet(&mut app, named(0x9, "Rik", 1));
        assert_eq!(
            packet(&mut app, log(true, vec![score(0x9, 3, 0, 0)])),
            [BattlefieldEvent::Score]
        );
        let now = Instant::now();
        {
            let mut board = app.world_mut().resource_mut::<BattlefieldScoreboard>();
            assert!(board.request_due(now));
            let s = board.summary();
            assert_eq!((s.filtered, s.ended, s.winner), (1, true, 1));
        }
        world_enter(&mut app);
        let mut board = app.world_mut().resource_mut::<BattlefieldScoreboard>();
        let s = board.summary();
        assert_eq!((s.filtered, s.ended), (0, false));
        assert!(board.rows.is_empty());
        assert!(
            board.request_due(now),
            "the stamp is zeroed, so the next request is not throttled"
        );
    }

    /// The world-enter cascade's battlefield init (`0x4a9db0`).
    fn world_enter(app: &mut App) {
        app.world_mut()
            .run_system_cached(crate::ui_battlefield::reset_on_world_enter)
            .expect("the battlefield init runs");
    }

    /// The board's guids in board order.
    fn order(app: &App) -> Vec<u64> {
        let board = app.world().resource::<BattlefieldScoreboard>();
        board.rows.iter().map(|r| r.guid).collect()
    }

    /// The comparator `0x4aa350` as the bytes write it, over a table of each guid's record (its
    /// name and side) and its block: 0 when either record is missing (`0x4aa3c9`); with a filter,
    /// two sides apart put the filter's first and answer +1 both ways when neither is it
    /// (`0x4aa4f5`-`0x4aa4fa`); then kills down, deaths up, honor down, `SStrCmpI` (`0x4aa567`).
    fn reference_cmp(
        filter: i8,
        records: &std::collections::HashMap<u64, (String, i8)>,
        a: &(u64, u32, u32, u32),
        b: &(u64, u32, u32, u32),
    ) -> i32 {
        let (Some((na, ta)), Some((nb, tb))) = (records.get(&a.0), records.get(&b.0)) else {
            return 0;
        };
        if filter != -1 && ta != tb {
            return if *ta != filter { 1 } else { -1 };
        }
        let key = |x: u32, y: u32, up: bool| match (x == y, (y < x) == up) {
            (true, _) => None,
            (false, true) => Some(-1),
            (false, false) => Some(1),
        };
        key(a.1, b.1, true)
            .or_else(|| key(a.2, b.2, false))
            .or_else(|| key(a.3, b.3, true))
            .unwrap_or_else(|| benilla_ui::crt::str_cmp_i(na, nb))
    }

    /// The rebuild sorts with the CRT `qsort` (`0x73f727` at `0x4aa2fb`) under the reference
    /// comparator, which orders no row whose record is missing: a 40-row board with refused names
    /// and case-folded ties comes out in qsort's own order, with and without a filter. A row with
    /// no record is team 0 (`0x4aa26d` → `0x4aa2b5`), so filter 0 counts it.
    #[test]
    fn the_board_sorts_as_qsort_runs_the_reference_comparator() {
        let (mut app, _rx) = board();
        let names = ["Ann", "ann", "Bob", "bob", "Cid", "dee", "Eve"];
        let mut records = std::collections::HashMap::new();
        let mut blocks = Vec::new();
        let mut log_rows = Vec::new();
        for i in 0..40u32 {
            let guid = 0x100 + u64::from(i);
            let (kills, deaths, honor) = ((i * 7) % 3, (i * 3) % 2, (i % 2) * 10);
            blocks.push((guid, kills, deaths, honor));
            log_rows.push(score(guid, kills, deaths, honor));
        }
        let mut answers = Vec::new();
        for i in 0..40u32 {
            let guid = 0x100 + u64::from(i);
            if i % 6 == 0 {
                answers.push(named(guid, "", 0));
            } else {
                let race = if i % 3 == 1 { 1 } else { 2 };
                let side = if race == 1 { 1 } else { 0 };
                let name = names[i as usize % names.len()];
                records.insert(guid, (name.to_string(), side));
                answers.push(named(guid, name, race));
            }
        }
        packet(&mut app, log(false, log_rows));
        for a in answers {
            packet(&mut app, a);
        }
        let mut want = blocks.clone();
        benilla_ui::crt::qsort(&mut want, |a, b| reference_cmp(-1, &records, a, b));
        assert_eq!(order(&app), want.iter().map(|b| b.0).collect::<Vec<_>>());

        let mut reads = bevy::ecs::system::SystemState::<BoardReads>::new(app.world_mut());
        let mut filter = |app: &mut App, f: i8| {
            let world = app.world_mut();
            world.resource_scope(|world, mut board: Mut<BattlefieldScoreboard>| {
                board.set_filter(f, &reads.get(world))
            })
        };
        assert!(filter(&mut app, 1));
        benilla_ui::crt::qsort(&mut want, |a, b| reference_cmp(1, &records, a, b));
        assert_eq!(order(&app), want.iter().map(|b| b.0).collect::<Vec<_>>());

        assert!(filter(&mut app, 0));
        let board = app.world().resource::<BattlefieldScoreboard>();
        let unnamed = board.rows.iter().find(|r| r.guid == 0x100).expect("row 0");
        assert_eq!(unnamed.team, 0, "no record, team 0");
        let horde = records.values().filter(|(_, t)| *t == 0).count();
        assert_eq!(
            app.world()
                .resource::<BattlefieldScoreboard>()
                .summary()
                .filtered,
            horde + 7,
            "the seven refused rows count under filter 0"
        );
    }

    /// The world-enter cascade zeroes the stat count `0xb6ebd8` with the board (`0x4a9e67`).
    #[test]
    fn a_world_enter_clears_the_stat_columns() {
        let (mut app, _rx) = board();
        app.world_mut()
            .resource_mut::<BattlefieldScoreboard>()
            .columns = vec![BattlefieldStatColumn::default()];
        world_enter(&mut app);
        let board = app.world().resource::<BattlefieldScoreboard>();
        assert_eq!(board.summary().columns, 0);
    }

    #[test]
    fn the_session_end_zeroes_the_scoreboard() {
        let (mut app, _rx) = board();
        packet(&mut app, log(true, Vec::new()));
        app.world_mut()
            .resource_mut::<BattlefieldScoreboard>()
            .last_request = Some(Instant::now());

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );

        let board = app.world().resource::<BattlefieldScoreboard>();
        assert!(!board.ended);
        assert!(board.last_request.is_none());
    }
}
