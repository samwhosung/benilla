//! The app's half of the channel verbs ([`benilla_ui::script::ChannelCommand`]), which the call
//! host applies inside each call ([`super::ChatSender::channel`]): the slot table the reference's
//! verbs read and write, and the packets they send.

use benilla_ui::script::{ChannelCommand, UiScript};

use super::super::edit::ChannelState;
use super::super::feed::ChatLog;
use crate::net::{ClientCommand, NetCommands};

/// Apply one queued channel verb: its slot writes and its packet, or its chat line.
pub(in crate::ui_chat) fn apply(
    cmd: ChannelCommand,
    channels: &mut ChannelState,
    script: &mut UiScript,
    commands: &NetCommands,
    chat_log: &ChatLog,
    errors: &crate::ui_action::UiErrorKeys,
) {
    use ChannelCommand as C;
    let cmd = match cmd {
        C::Join { name, password, id } => {
            // The join body takes the slot before it builds the packet (`0x49edf3`), and sends
            // whether or not one was free (`0x49ee02`).
            if channels.claim_or_refuse(&name, id, errors).is_some() {
                channels.mirror_into(script);
            }
            ClientCommand::JoinChannel { name, password }
        }
        C::Leave { name } => {
            // `LeaveChannelByName` (`0x4a0000` → `0x49ee70`): a number names a confirmed slot or
            // the call does nothing. Only a leave clears the zone mask and the custom re-join
            // list, the walk's own LEAVE never.
            let Some(name) = channels.channel_key(&name) else {
                return;
            };
            channels.note_zone_channel_left(&name);
            channels.note_custom_channel_left(&name);
            ClientCommand::LeaveChannel { name }
        }
        C::ListAll => {
            // The line is added inside the verb (`0x4a02c0` → `0x49a870`), so a handler has run
            // by the verb's next line.
            chat_log.show_now(script, &list_channels(channels));
            return;
        }
        admin => {
            // The admin verbs pass argument 1 through `ParseChannelKey` (`0x4a014f`, `0x4a03df`,
            // `0x4a04f5`); a number naming no confirmed slot ends the call before the packet
            // (`0x4a015a`, `0x4a03ea`, `0x4a0500`).
            let Some(cmd) = admin_command(admin, |key| channels.channel_key(key)) else {
                return;
            };
            cmd
        }
    };
    let _ = commands.0.send(cmd);
}

/// `ListChannels` (`0x4a01c0`): `"[%d. %s] "` (`0x844c94`) for each slot in state 0
/// (`0x4a01f2`-`0x4a01fb`), as one `CHAT_MSG_CHANNEL_LIST` line (type `0x11`) with no channel,
/// empty or not (`0x4a02a1`-`0x4a02c0`); ten entries of at most `0x89` bytes fit its 2048-byte
/// line.
fn list_channels(channels: &ChannelState) -> super::super::event::ChatEvent {
    let text: String = channels
        .confirmed_slots()
        .map(|(n, slot)| format!("[{n}. {}] ", slot.name))
        .collect();
    super::super::event::ChatEvent::text_only(super::super::event::ChatEventKind::ChannelList, text)
}

/// The packet of an admin verb, its channel argument resolved by `key`; `None` when it resolves
/// to nothing.
fn admin_command(
    cmd: ChannelCommand,
    key: impl Fn(&str) -> Option<String>,
) -> Option<ClientCommand> {
    use ChannelCommand as C;
    use ClientCommand as W;
    Some(match cmd {
        C::List { name } => W::ChannelList { name: key(&name)? },
        C::DisplayOwner { name } => W::ChannelOwner { name: key(&name)? },
        C::Moderate { name } => W::ChannelModerate { name: key(&name)? },
        C::ToggleAnnouncements { name } => W::ChannelAnnouncements { name: key(&name)? },
        C::SetOwner { name, player } => W::ChannelSetOwner {
            name: key(&name)?,
            player,
        },
        C::SetPassword { name, password } => W::ChannelPassword {
            name: key(&name)?,
            password,
        },
        C::Ban { name, player } => W::ChannelBan {
            name: key(&name)?,
            player,
        },
        C::Invite { name, player } => W::ChannelInvite {
            name: key(&name)?,
            player,
        },
        C::Kick { name, player } => W::ChannelKick {
            name: key(&name)?,
            player,
        },
        C::Moderator { name, player } => W::ChannelModerator {
            name: key(&name)?,
            player,
        },
        C::Unmoderator { name, player } => W::ChannelUnmoderator {
            name: key(&name)?,
            player,
        },
        C::Mute { name, player } => W::ChannelMute {
            name: key(&name)?,
            player,
        },
        C::Unmute { name, player } => W::ChannelUnmute {
            name: key(&name)?,
            player,
        },
        C::Unban { name, player } => W::ChannelUnban {
            name: key(&name)?,
            player,
        },
        C::Join { .. } | C::Leave { .. } | C::ListAll => return None,
    })
}
