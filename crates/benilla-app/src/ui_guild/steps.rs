//! The guild verbs inside their calls ([`GuildSteps`]): each sends its packet, or re-sorts the
//! roster and signals `GUILD_ROSTER_UPDATE`, before the verb returns, as the reference's bindings
//! do (`0x48aef0`-`0x48b410`, `0x4d10d0`, `0x4d0fb0`, `0x4d0f70`, `0x4d15e0`, `0x4d20d0`).

use benilla_protocol::messages::{GUILD_RANKS_MAX_COUNT, GUILD_RANKS_MIN_COUNT};
use benilla_ui::script::{GuildCall, UiScript};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::{feed, live, GuildLocal, GuildState, ROSTER_REQUEST_THROTTLE_SECS};
use crate::area::AreaTableRes;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_unit::NameTables;

/// The roster, the state its verbs write and what they send through: the engine half of the guild
/// calls ([`crate::script_calls::ScriptCallHost`]). The roster itself is only read, as of the
/// place the call runs ([`GuildState::seen`]): its notes change with the server's next roster,
/// never with a verb (`0x4d15e0`).
#[derive(SystemParam)]
pub(crate) struct GuildSteps<'w> {
    /// Absent, with [`Self::local`], in a world with no guild windows, which drops the calls.
    guild: Option<Res<'w, GuildState>>,
    local: Option<ResMut<'w, GuildLocal>>,
    /// The tables the order names the zones and classes from.
    areas: Option<Res<'w, AreaTableRes>>,
    unit_names: NameTables<'w>,
    /// The notes' masker: a setter's compare reads them through `0x4d09b0`, which masks them.
    masker: super::Masker<'w>,
    net: Res<'w, NetCommands>,
    /// The roster request's deadline clock. Real time: the virtual clock's 250 ms `max_delta`
    /// lags in a hitch.
    time: Option<Res<'w, Time<Real>>>,
}

impl GuildSteps<'_> {
    /// Apply `call` inside its verb.
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: GuildCall) {
        let Self {
            guild,
            local,
            areas,
            unit_names,
            masker,
            net,
            time,
        } = self;
        let (Some(guild), Some(local)) = (guild.as_deref(), local.as_deref_mut()) else {
            debug!("ui_guild: no guild state in this world; {call:?} dropped");
            return;
        };
        let send = |command| {
            let _ = net.0.send(command);
        };
        // The re-sort's bare `GUILD_ROSTER_UPDATE` (`0x4d1160(0)`), in the call; the getters read
        // the order the chain and show-offline now give.
        let resorted = |script: &mut UiScript| {
            script.fire_event("GUILD_ROSTER_UPDATE", vec![]);
        };
        // The row at a 1-based display index (`[0xb72704]` at `index - 1`, `0x4d1633`), over the
        // whole roster, not the show-offline count.
        let row = |script: &UiScript, local: &GuildLocal, index: u32| {
            let rows = local.ordered(
                &guild.seen().roster,
                script.guild_show_offline() != 0,
                areas.as_deref().map(|a| &a.0),
                unit_names.get(),
                &|text| masker.cached(text),
            );
            live::row_at(&rows, i64::from(index)).cloned()
        };
        match call {
            // One request per 10 000 ms, the rest swallowed silently (`0x4d10d0`, against
            // `0xb73130`).
            GuildCall::Roster => {
                let now = time.as_ref().map_or(0.0, |t| t.elapsed_secs_f64());
                if now >= guild.roster_allowed_at() {
                    guild.defer_roster_request(now + ROSTER_REQUEST_THROTTLE_SECS);
                    send(ClientCommand::GuildRosterRequest);
                }
            }
            GuildCall::Info => send(ClientCommand::GuildInfoRequest),
            GuildCall::Invite(name) => send(ClientCommand::GuildInvite { name }),
            GuildCall::Uninvite(name) => send(ClientCommand::GuildRemove { name }),
            GuildCall::Promote(name) => send(ClientCommand::GuildPromote { name }),
            GuildCall::Demote(name) => send(ClientCommand::GuildDemote { name }),
            GuildCall::SetLeader(name) => send(ClientCommand::GuildLeader { name }),
            GuildCall::Accept => send(ClientCommand::GuildAccept),
            GuildCall::Decline => send(ClientCommand::GuildDecline),
            GuildCall::Leave => send(ClientCommand::GuildLeave),
            GuildCall::Disband => send(ClientCommand::GuildDisband),
            GuildCall::SetMotd(motd) => send(ClientCommand::GuildMotd { motd }),
            // No local write: `SetGuildInfoText 0x4d2380` only sends, leaving `0xb72720` to change
            // with the next roster.
            GuildCall::SetInfoText(text) => send(ClientCommand::GuildInfoText { text }),
            // The row's member (`0x4d1633`), sent only when the text differs from its stored note
            // (`0x64a480` at `0x4d1668`), which stays as it is.
            GuildCall::SetPublicNote { index, note } => {
                if let Some(m) = row(script, local, index) {
                    if m.info.note != note {
                        let name = m.info.name;
                        send(ClientCommand::GuildSetPublicNote { name, note });
                    }
                }
            }
            GuildCall::SetOfficerNote { index, note } => {
                if let Some(m) = row(script, local, index) {
                    if m.info.officer_note != note {
                        let name = m.info.name;
                        send(ClientCommand::GuildSetOfficerNote { name, note });
                    }
                }
            }
            GuildCall::SaveRank {
                rank_index,
                rights,
                name,
            } => {
                if let Some(name) = feed::capped_rank_name(name) {
                    send(ClientCommand::GuildRank {
                        rank_id: rank_index,
                        rights,
                        name,
                    });
                }
            }
            // Ten ranks at most, and more than five to remove one, as the reference's
            // `GuildControlAddRank` (`0x4d2271`) and `GuildControlDelRank` (`0x4d22e6`) check
            // silently; the stock buttons follow them (`FriendsFrame.lua:887`, `:908`).
            GuildCall::AddRank(name) => {
                if guild.seen().roster.rank_rights.len() < GUILD_RANKS_MAX_COUNT {
                    if let Some(name) = feed::capped_rank_name(name) {
                        send(ClientCommand::GuildAddRank { name });
                    }
                }
            }
            GuildCall::DelRank => {
                if guild.seen().roster.rank_rights.len() > GUILD_RANKS_MIN_COUNT {
                    send(ClientCommand::GuildDelRank);
                }
            }
            // `0x4d0f70` past its compare and its store, which the binding made: with a member held
            // the array is sorted and the roster signalled (`0x4d0f85`-`0x4d0f9f`).
            GuildCall::SetShowOffline => {
                if !guild.seen().roster.members.is_empty() {
                    resorted(script);
                }
            }
            // `0x4d0fb0`: the chain's step, the sort and the signal, member or none (`0x4d1022`).
            GuildCall::Sort(field) => {
                local.sort_by(&field);
                resorted(script);
            }
        }
    }
}
