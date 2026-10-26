//! The gossip getters' reads at the call, the call host's half of
//! [`benilla_ui::script::GossipQuery`]: the message's slots and the drawn text as the handlers and
//! the selects left them (`0xbbbe90`, `0xbb74c0`, `0xbbb678`), which a close leaves for the getters
//! (`0x4e1fa0`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::GossipQuery;

use super::GossipState;

/// What the gossip getters read as they run; the store absent in a harness without the gossip
/// window, where the getters read the VM's copy.
#[derive(SystemParam)]
pub(crate) struct GossipLive<'w> {
    state: Option<Res<'w, GossipState>>,
}

impl crate::script_calls::live::LiveFamily for GossipLive<'_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|_: &GossipQuery| self.state.as_deref().map(super::menu))
    }
}
