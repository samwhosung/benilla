//! The designer's reads at the call, the call host's half of
//! [`benilla_ui::script::TabardHostQuery`]: the guild record cached under the active player's
//! `PLAYER_GUILDID` (`0x560e30` on `0xc0e1ec`, `0x5f0856`) and the save latch `0xc4d780`
//! (`0x5f085f`), as `CanSaveTabardNow` (`0x5f0800`) and `InitializeTabardColors` (`0x5028e0`)
//! read them.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{TabardHost, TabardHostQuery};

use super::TabardOpen;
use crate::net::Objects;
use crate::ui_guild::GuildState;

/// What the designer's getters read as they run; each absent in a harness without it, where they
/// read the VM's copy.
#[derive(SystemParam)]
pub(crate) struct TabardLive<'w, 's> {
    open: Option<Res<'w, TabardOpen>>,
    guilds: Option<Res<'w, GuildState>>,
    objects: Objects<'w, 's>,
}

impl TabardLive<'_, '_> {
    /// The record and the latch now; with no player, no guild id and so no record.
    fn host(&self) -> Option<TabardHost> {
        let (open, guilds) = (self.open.as_deref()?, self.guilds.as_deref()?);
        let guild_id = self
            .objects
            .player_fields()
            .map_or(0, |p| p.player_guild_id());
        Some(TabardHost {
            guild_record: guilds.own_emblem_record(guild_id),
            save_pending: open.save_pending,
        })
    }
}

impl crate::script_calls::live::LiveFamily for TabardLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|_: &TabardHostQuery| self.host())
    }
}
