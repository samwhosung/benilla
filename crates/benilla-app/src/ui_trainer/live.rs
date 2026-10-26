//! The trainer getters' per-call reads, the call host's half of
//! [`benilla_ui::script::TrainerServiceQuery`]: `GetTrainerServiceIcon`'s product lookup
//! (`0x4d9096`), `GetTrainerServiceDescription`'s text and its product lookup (`0x4d9d07`, the
//! expander `0x5075f0` over the caster now) and `GetTrainerServiceAbilityReq`'s book test
//! (`0x4d97c8` → `0x60c740`, `0x60c8d0`), over the record the last list or pass left.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::TrainerServiceQuery;

use super::{TrainerOpen, TrainerReads};

/// What the getters read as they run; the window absent in a harness without it, where the getters
/// read the VM's copy.
#[derive(SystemParam)]
pub(crate) struct TrainerLive<'w, 's> {
    open: Option<Res<'w, TrainerOpen>>,
    reads: TrainerReads<'w, 's>,
}

impl crate::script_calls::live::LiveFamily for TrainerLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|q: &TrainerServiceQuery| {
            // A list landed since the VM's rows were pushed: they index the list before it.
            let open = self.open.as_deref().filter(|o| o.list == q.list)?;
            self.reads.live(open, q.service)
        })
    }
}
