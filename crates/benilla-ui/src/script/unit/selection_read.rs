//! The selection's and the interaction's globals, asked of the lent host at the call
//! ([`crate::script::live::ask`]): the resolver reads the target global `[0xb4e2d8]` (`0x515a86`)
//! and the interaction global `[0xb4e2d0]` (`0x515c00`) as they stand when the getter runs, so a
//! listener reads the selection its own packet left. With no host free to ask, the snapshot
//! ([`super::UnitGuids`]) answers.

use mlua::Lua;

use super::super::live::{ask, LiveQuery};

/// The two globals the resolver reads for `"target"` and `"npc"`, 0 for nobody.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionGlobals {
    /// The target global `[0xb4e2d8]`, a guid whether or not its object is held: the resolver
    /// tests nothing before it returns it (`0x515a86`).
    pub target: u64,
    /// The interaction global `[0xb4e2d0]`, likewise returned untested (`0x515c00`).
    pub npc: u64,
}

/// The question: the two globals as of the call.
pub struct SelectionQuery;

impl LiveQuery for SelectionQuery {
    type Answer = SelectionGlobals;
}

/// The globals through the lent host, `None` with no host free to ask or one that declines.
pub(crate) fn live_selection(lua: &Lua) -> Option<SelectionGlobals> {
    ask(lua, &SelectionQuery)
}

/// The name `UnitName` reads off the object held under a guid that is no unit (`0x517146`): a game
/// object's record name, `""` with no record (`0x517152` → `0x5f8100`), and an item's cached
/// template name (`0x517178`), `None` on a miss, which reads `UNKNOWNOBJECT` (`0x517213`).
/// Declined for a unit, a guid the manager does not hold and any other object.
pub struct ObjectNameQuery(pub u64);

impl LiveQuery for ObjectNameQuery {
    type Answer = Option<String>;
}
