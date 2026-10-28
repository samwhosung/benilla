//! The targeting word as the VM's tests read it at the call, the call host's half of their
//! questions: `SpellCanTargetUnit`'s (`0x6e6d00` → `0x6e6460`, the range included) and the
//! pickups' and the minimap ping's halves of the word (`0x6e6330`, `0x6e6320`), each read off the
//! word, the unit and its range as they stand.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{SpellCanTargetQuery, TargetingWants as Wants, TargetingWantsQuery};

use super::{BindChecks, SpellTargeting, TargetingWants};

/// The word, the unit checks and the units by guid; absent in a harness without them, where the
/// tests read the VM's copy.
#[derive(SystemParam)]
pub(crate) struct TargetingLive<'w, 's> {
    targeting: Res<'w, SpellTargeting>,
    checks: BindChecks<'w, 's>,
    /// The object lookup the guid goes through (`0x468460`).
    index: Res<'w, crate::net::GuidIndex>,
}

impl crate::script_calls::live::LiveFamily for TargetingLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|_: &TargetingWantsQuery| {
            Some(Wants {
                item: self.targeting.wants(TargetingWants::Item),
                location: self.targeting.wants(TargetingWants::Location),
            })
        }) || ask.answer(|q: &SpellCanTargetQuery| {
            Some(
                self.index
                    .0
                    .get(&q.0)
                    .is_some_and(|&entity| self.targeting.can_target_unit(entity, &self.checks)),
            )
        })
    }
}
