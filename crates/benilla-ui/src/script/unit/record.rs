//! The name caches' reads at the call. A getter that names a unit or reads its creature record
//! asks the lent host ([`crate::script::live::ask`]) and reads the caches as they stand where it
//! runs: `UnitName` names a unit through `GetUnitName` (`0x609210`) and `UnitCreatureType`,
//! `UnitClassification`, `UnitLevel`'s world-boss leg and the civilian gate read the unit's cached
//! creature record (`[unit+0xb30]`, `0x605570`, `0x605620`, `0x612550`). With no host free to ask,
//! the frame's snapshot answers.

use mlua::Lua;

use super::{view, UnitState};
use crate::script::live::{ask, LiveQuery};
use crate::script::Model;

/// The player-name cache's record for a guid as a getter reads it, reading only (`0x55f080` on a
/// valid node): the answer is `None` for a miss and a refusal, and the query itself is declined
/// (no answer) with no cache to ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerNameQuery(pub u64);

impl LiveQuery for PlayerNameQuery {
    type Answer = Option<String>;
}

/// The creature-record legs of the unit object held under a guid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitRecordQuery(pub u64);

impl LiveQuery for UnitRecordQuery {
    type Answer = UnitRecord;
}

/// What the unit getters read of a unit's creature record, as of the place they run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitRecord {
    /// [`UnitState::creature_type_name`].
    pub creature_type: Option<String>,
    /// [`UnitState::rank`]: the gated rank (`0x605620`).
    pub rank: u32,
    /// [`UnitState::civilian`].
    pub civilian: bool,
}

impl UnitRecord {
    /// The record legs a snapshot holds as its build read them.
    fn of_snapshot(unit: &UnitState) -> Self {
        UnitRecord {
            creature_type: unit.creature_type_name.clone(),
            rank: unit.rank,
            civilian: unit.civilian,
        }
    }
}

/// The unit `token` names, mapped through `f` with its snapshot and its creature record; `None`
/// for no unit. The token resolves at the call ([`view`]); a held unit's record legs are read off
/// the name caches through the lent host, so a record that landed after the frame's push, or for a
/// unit no snapshot holds, answers; with no host free to ask, or no object held, the snapshot does.
pub(crate) fn read_record<T>(
    lua: &Lua,
    token: &str,
    f: impl FnOnce(&UnitState, &UnitRecord) -> T,
) -> Option<T> {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let unit = view(lua, &model, token)?;
    let live = (unit.has_object && unit.guid != 0)
        .then(|| ask(lua, &UnitRecordQuery(unit.guid)))
        .flatten();
    let record = live.unwrap_or_else(|| UnitRecord::of_snapshot(&unit));
    Some(f(&unit, &record))
}
