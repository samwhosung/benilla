//! What the battleground getters ask the lent host as they run ([`super::live::ask`]), each as of
//! the place the getter runs. The reference's getters read the module's globals at the call: the
//! queue slots (`0xb6e9d0`, `0x4ab4a0`), the instance clocks (`0xb6ebb8`, `0xb6ebbc`), the list
//! (`0xb6e860`, `0x4ab0b0`), the board (`0xb6e880`, `0x4ab9d0`) and the position arrays
//! (`0xb6e710`, `0x4abf90`), which the packet handlers write inside themselves, so a listener reads
//! what its own packet left. With no host to ask, the VM's copy answers; only a test sets it, as
//! every production entry that runs Lua lends the host.

use mlua::Lua;

use super::live::{ask, LiveQuery};
use super::{
    BattlefieldFlagView, BattlefieldListView, BattlefieldPositionView, BattlefieldQueueSlot,
    BattlefieldScoreRow, BattlefieldStatColumn, Model,
};

/// Queue slot `0..3` (`0xb6e9d0`, stride `0x20`), its clocks reduced at the call.
pub struct BattlefieldSlotQuery(pub usize);

impl LiveQuery for BattlefieldSlotQuery {
    type Answer = BattlefieldQueueSlot;
}

/// The instance's two clocks at the call, in ms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BattlefieldTimes {
    /// `[0xb6ebb8] − now`, 0 when unset or past.
    pub instance_expiration_ms: u32,
    /// `now − [0xb6ebbc]`, 0 when unset.
    pub run_time_ms: u32,
}

/// The question: [`BattlefieldTimes`] now.
pub struct BattlefieldTimesQuery;

impl LiveQuery for BattlefieldTimesQuery {
    type Answer = BattlefieldTimes;
}

/// The instance list and its map row (`0xb6e860`, `[0xb6eba4]`), the description picked by the
/// player's faction group at the call (`0x5efe00`).
pub struct BattlefieldListQuery;

impl LiveQuery for BattlefieldListQuery {
    type Answer = BattlefieldListView;
}

/// The board's scalars at the call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BattlefieldScoreSummary {
    /// The filtered count (`0xb6ebc4`).
    pub filtered: usize,
    /// The ended flag (`0xb6ebcc`) and the winner (`0xb6ebd0`).
    pub ended: bool,
    pub winner: u8,
    /// The stat columns' count (`0xb6ebd8`).
    pub columns: usize,
}

/// The question: [`BattlefieldScoreSummary`] now.
pub struct BattlefieldScoreSummaryQuery;

impl LiveQuery for BattlefieldScoreSummaryQuery {
    type Answer = BattlefieldScoreSummary;
}

/// The 1-based row of the sorted board (`[0xb6e880 + 4·(i − 1)]`), bounded by the wire count
/// (`0xb6ebc0`), with its name, race and class read off the name cache at the call; `None` past
/// the count (`0x4ab9d0`, `0x4abdc0`).
pub struct BattlefieldScoreRowQuery(pub i32);

impl LiveQuery for BattlefieldScoreRowQuery {
    type Answer = Option<BattlefieldScoreRead>;
}

/// A board row as the getters read it at the call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BattlefieldScoreRead {
    pub row: BattlefieldScoreRow,
    /// Whether the name cache held the row's record: without it `GetBattlefieldScore` answers its
    /// fail leg (`0x4aba77`), while `GetBattlefieldStatData` reads the block alone (`0x4abe20`).
    pub named: bool,
}

/// The 1-based stat column (`0xb6e6ec`), `None` past the count.
pub struct BattlefieldStatColumnQuery(pub i32);

impl LiveQuery for BattlefieldStatColumnQuery {
    type Answer = Option<BattlefieldStatColumn>;
}

/// The stored teammates that are neither us nor a party slot nor on the raid roster (`0x4abf00`).
pub struct BattlefieldPositionCountQuery;

impl LiveQuery for BattlefieldPositionCountQuery {
    type Answer = usize;
}

/// The 1-based survivor of [`BattlefieldPositionCountQuery`]'s filter, projected onto the map the
/// world map shows, `selection` ([`super::UiScript::world_map_selection`]); `None` off the list
/// (`0x4abf90`).
pub struct BattlefieldPositionQuery {
    pub index: i32,
    pub selection: (u32, u32, Option<u32>),
}

impl LiveQuery for BattlefieldPositionQuery {
    type Answer = Option<BattlefieldPositionView>;
}

/// The flag carrier (`0xb6e6d8`), projected as [`BattlefieldPositionQuery`] does; `None` with no
/// carrier (`0x4ac1c0`, `0x4ac230`).
pub struct BattlefieldFlagQuery {
    pub selection: (u32, u32, Option<u32>),
}

impl LiveQuery for BattlefieldFlagQuery {
    type Answer = Option<BattlefieldFlagView>;
}

/// The active slot's map row's icon scale, map 0's with no slot, 1.0 for a missing row
/// (`0x4ac3d0`).
pub struct BattlefieldIconScaleQuery;

impl LiveQuery for BattlefieldIconScaleQuery {
    type Answer = f32;
}

/// Slot `i` (0-based) now, `None` past the three.
pub(super) fn slot(lua: &Lua, model: &Model, i: usize) -> Option<BattlefieldQueueSlot> {
    if i >= 3 {
        return None;
    }
    ask(lua, &BattlefieldSlotQuery(i)).or_else(|| model.battlefield_slots.get(i).cloned())
}

/// The instance clocks now.
pub(super) fn times(lua: &Lua, model: &Model) -> BattlefieldTimes {
    ask(lua, &BattlefieldTimesQuery).unwrap_or(BattlefieldTimes {
        instance_expiration_ms: model.battlefield_instance_expiration_ms,
        run_time_ms: model.battlefield_run_time_ms,
    })
}

/// The list now.
pub(super) fn list(lua: &Lua, model: &Model) -> BattlefieldListView {
    ask(lua, &BattlefieldListQuery).unwrap_or_else(|| model.battlefield_list.clone())
}

/// The board's scalars now.
pub(super) fn score_summary(lua: &Lua, model: &Model) -> BattlefieldScoreSummary {
    ask(lua, &BattlefieldScoreSummaryQuery).unwrap_or_else(|| {
        let board = &model.battlefield_board;
        BattlefieldScoreSummary {
            filtered: board.filtered,
            ended: board.scores.ended,
            winner: board.scores.winner,
            columns: board.scores.columns.len(),
        }
    })
}

/// Row `index` (1-based) of the sorted board now; the VM's copy holds named rows alone.
pub(super) fn score_row(lua: &Lua, model: &Model, index: i32) -> Option<BattlefieldScoreRead> {
    ask(lua, &BattlefieldScoreRowQuery(index)).unwrap_or_else(|| {
        model
            .battlefield_board
            .row(index)
            .map(|row| BattlefieldScoreRead {
                row: row.clone(),
                named: true,
            })
    })
}

/// Stat column `index` (1-based) now.
pub(super) fn stat_column(lua: &Lua, model: &Model, index: i32) -> Option<BattlefieldStatColumn> {
    ask(lua, &BattlefieldStatColumnQuery(index)).unwrap_or_else(|| {
        usize::try_from(index)
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| model.battlefield_board.scores.columns.get(i))
            .cloned()
    })
}

/// The teammate count now.
pub(super) fn position_count(lua: &Lua, model: &Model) -> usize {
    ask(lua, &BattlefieldPositionCountQuery).unwrap_or(model.battlefield_positions.len())
}

/// Teammate `index` (1-based) now, under the map the world map shows.
pub(super) fn position(lua: &Lua, model: &Model, index: i32) -> Option<BattlefieldPositionView> {
    let selection = world_map_selection(model);
    ask(lua, &BattlefieldPositionQuery { index, selection }).unwrap_or_else(|| {
        usize::try_from(index)
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| model.battlefield_positions.get(i))
            .cloned()
    })
}

/// The flag carrier now.
pub(super) fn flag(lua: &Lua, model: &Model) -> Option<BattlefieldFlagView> {
    let selection = world_map_selection(model);
    ask(lua, &BattlefieldFlagQuery { selection }).unwrap_or_else(|| model.battlefield_flag.clone())
}

/// The icon scale now.
pub(super) fn icon_scale(lua: &Lua, model: &Model) -> f32 {
    ask(lua, &BattlefieldIconScaleQuery).unwrap_or(model.battlefield_icon_scale)
}

/// The world map's displayed map, as [`super::UiScript::world_map_selection`] reads it.
fn world_map_selection(model: &Model) -> (u32, u32, Option<u32>) {
    let wm = &model.worldmap;
    (wm.selection.0, wm.selection.1, wm.direct_area)
}
