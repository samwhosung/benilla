//! The relation functions as the Lua API asks them ([`UnitRelations`]): every unit the object
//! manager holds, copied with the tables they are judged by, so the six relation verbs answer the
//! model's verdict for any two of them, in the order the tokens come. `ui_unit`'s feed pushes it.

use std::collections::HashMap;

use benilla_ui::script::UnitRelations;

use super::{RelUnit, Relations};
use crate::net::Reputations;
use crate::spell::group_relation::GroupRoster;
use crate::target::Factions;

/// The held units by guid and the state the relation functions read beside them.
pub(crate) struct RelationSnapshot {
    pub(crate) units: HashMap<u64, RelUnit>,
    pub(crate) factions: Option<Factions>,
    pub(crate) reputations: Reputations,
    pub(crate) me: Option<u64>,
    pub(crate) roster: GroupRoster,
}

impl RelationSnapshot {
    fn rel(&self) -> Relations<'_> {
        Relations {
            factions: self.factions.as_ref(),
            reputations: &self.reputations,
            me: self.me,
            roster: Some(&self.roster),
            units: &self.units,
        }
    }

    /// Both units, `None` when either is not held.
    fn pair(&self, a: u64, b: u64) -> Option<(&RelUnit, &RelUnit)> {
        Some((self.units.get(&a)?, self.units.get(&b)?))
    }
}

impl UnitRelations for RelationSnapshot {
    fn holds(&self, guid: u64) -> bool {
        self.units.contains_key(&guid)
    }

    fn reaction(&self, a: u64, b: u64) -> Option<u8> {
        let (a, b) = self.pair(a, b)?;
        Some(self.rel().reaction(a, b))
    }

    fn can_attack(&self, a: u64, b: u64) -> Option<bool> {
        let (a, b) = self.pair(a, b)?;
        Some(self.rel().can_attack(a, b))
    }

    fn can_assist(&self, a: u64, b: u64) -> Option<bool> {
        let (a, b) = self.pair(a, b)?;
        Some(self.rel().can_assist(a, b))
    }

    fn can_cooperate(&self, a: u64, b: u64) -> Option<bool> {
        let (a, b) = self.pair(a, b)?;
        Some(self.rel().can_cooperate(a, b))
    }

    fn is_pvp(&self, guid: u64) -> Option<bool> {
        Some(self.rel().is_pvp(self.units.get(&guid)?))
    }
}
