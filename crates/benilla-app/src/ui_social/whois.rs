//! The `whois` console command.

use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands};

/// `FriendList::Initialize` registers it in category 0 with this help (`0x5adcfd`, `0x85dba8`).
pub(super) const HELP: &str =
    "Ask the server to do an account/real name lookup on a character name";

/// `SendWhoisRequest` (`0x5adee0`): `CMSG_WHOIS` with the whole argument text as one string
/// (`0x5adf1f`), the empty line too, whether or not a player is active. The answer is the
/// server's to give an administrator. Nothing is printed.
pub(super) fn whois(world: &mut World, args: &str) -> Vec<String> {
    if let Some(net) = world.get_resource::<NetCommands>() {
        let _ = net.0.send(ClientCommand::Whois {
            text: args.to_string(),
        });
    }
    Vec::new()
}
