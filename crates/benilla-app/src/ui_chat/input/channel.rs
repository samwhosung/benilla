//! The app's half of the channel verbs the VM queues ([`benilla_ui::script::ChannelCommand`]):
//! the slot table the reference's verbs read and write, and the packets they send.

use bevy::prelude::*;

use benilla_ui::script::{ChannelCommand, UiScript};

use super::super::edit::ChannelState;
use super::super::feed::ChatLog;
use crate::net::{ClientCommand, NetCommands};

/// A manual join or leave of `GuildRecruitment` turns its auto-join option off: `0x49ed3d` (join)
/// and `0x49ef8f` (leave) call `0x49ea70(0)` when the `ChatChannels.dbc` row has `flags & 0x20000`
/// and the caller passes its flag, which the Lua bindings do and the cascade's own calls do not.
fn manual_join_or_leave(channels: &ChannelState, script: &mut UiScript, wire_name: &str) {
    let guild_row = channels
        .channels
        .row_for_name(wire_name)
        .is_some_and(|r| r.is_guild_recruitment());
    if guild_row && script.reset_guild_recruitment_mode() {
        info!("chat: manual {wire_name:?} — auto-join guild recruitment channel switched off");
    }
}

/// Apply one queued channel verb: its slot writes and its packet, or its chat line.
pub(in crate::ui_chat) fn apply(
    cmd: ChannelCommand,
    channels: &mut ChannelState,
    script: &mut UiScript,
    commands: &NetCommands,
    chat_log: &ChatLog,
    _errors: &crate::ui_action::UiErrorKeys,
) {
    use ChannelCommand as C;
    let cmd = match cmd {
        C::Join { name, password, .. } => {
            manual_join_or_leave(channels, script, &name);
            ClientCommand::JoinChannel { name, password }
        }
        C::Leave { name } => {
            // `LeaveChannelByName` (`0x4a0000` → `0x49ee70`): a number names a confirmed slot or
            // the call does nothing. Only this path clears the zone mask and the custom re-join
            // list.
            let Some(name) = channels.leave_target(&name) else {
                return;
            };
            manual_join_or_leave(channels, script, &name);
            channels.note_zone_channel_left(&name);
            channels.note_custom_channel_left(&name);
            ClientCommand::LeaveChannel { name }
        }
        C::List { name } => ClientCommand::ChannelList { name },
        // `ListChannels()`: the joined roster, numbered as `/N` addresses it.
        C::ListAll => {
            let roster: Vec<String> = channels
                .joined
                .iter()
                .enumerate()
                .filter_map(|(i, c)| c.as_ref().map(|c| format!("{}. {}", i + 1, c.name)))
                .collect();
            let text = if roster.is_empty() {
                "You are not in any channels.".to_string()
            } else {
                format!("Channels: {}", roster.join(", "))
            };
            chat_log.push_event(super::super::event::ChatEvent::text_only(
                super::super::event::ChatEventKind::System,
                text,
            ));
            return;
        }
        C::DisplayOwner { name } => ClientCommand::ChannelOwner { name },
        C::SetOwner { name, player } => ClientCommand::ChannelSetOwner { name, player },
        C::SetPassword { name, password } => ClientCommand::ChannelPassword { name, password },
        C::Ban { name, player } => ClientCommand::ChannelBan { name, player },
        C::Invite { name, player } => ClientCommand::ChannelInvite { name, player },
        C::Kick { name, player } => ClientCommand::ChannelKick { name, player },
        C::Moderator { name, player } => ClientCommand::ChannelModerator { name, player },
        C::Unmoderator { name, player } => ClientCommand::ChannelUnmoderator { name, player },
        C::Mute { name, player } => ClientCommand::ChannelMute { name, player },
        C::Unmute { name, player } => ClientCommand::ChannelUnmute { name, player },
        C::Unban { name, player } => ClientCommand::ChannelUnban { name, player },
        C::Moderate { name } => ClientCommand::ChannelModerate { name },
        C::ToggleAnnouncements { name } => ClientCommand::ChannelAnnouncements { name },
    };
    let _ = commands.0.send(cmd);
}
