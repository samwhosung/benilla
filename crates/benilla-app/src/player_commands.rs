//! The console commands the reference registers with its player object: `RegisterPlayerCommands`
//! (`0x4049e0`), the first virtual of the object `ClientInitializeGame` builds (`0x401726`), runs
//! at start-up with no build or account test on the way, and `/console` reaches its commands
//! through `0x63ce00`, which has no category gate. Each is a send to the server; whether the
//! account may use it is the server's to say.

use bevy::prelude::*;

use crate::console::{ConsoleCommandApp, GAME};
use crate::net::{ClientCommand, NetCommands};

pub(crate) struct PlayerCommandsPlugin;

impl Plugin for PlayerCommandsPlugin {
    fn build(&self, app: &mut App) {
        // Category 4 with no help (`0x404a2e`-`0x404a48`).
        app.console_command("togglehelm", GAME, None, toggle_helm)
            .console_command("togglecloak", GAME, None, toggle_cloak);
    }
}

/// Queues `command` for the write thread, which drops it with no session as `NetClient::Send`
/// (`0x5379a0`) sends nothing unconnected. Nothing is printed.
fn send(world: &World, command: ClientCommand) -> Vec<String> {
    if let Some(net) = world.get_resource::<NetCommands>() {
        let _ = net.0.send(command);
    }
    Vec::new()
}

/// `ToggleHelmCommand` (`0x405250`): `CMSG_TOGGLE_HELM` with no body, whatever the arguments and
/// whether or not a player exists. The client's own flag is not consulted, unlike the interface's
/// Show Helm box (`objectlayer/inventory`, Helm and cloak).
fn toggle_helm(world: &mut World, _args: &str) -> Vec<String> {
    send(world, ClientCommand::ToggleHelm)
}

/// `ToggleCloakCommand` (`0x4052f0`): `CMSG_TOGGLE_CLOAK`, as [`toggle_helm`].
fn toggle_cloak(world: &mut World, _args: &str) -> Vec<String> {
    send(world, ClientCommand::ToggleCloak)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::{execute, ConsolePlugin};
    use crate::cvars::Cvars;
    use crossbeam_channel::Receiver;

    fn commands_app() -> (App, Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Cvars>()
            .insert_resource(NetCommands(tx))
            .add_plugins((ConsolePlugin, PlayerCommandsPlugin));
        (app, rx)
    }

    /// `0x405250` and `0x4052f0` each send one opcode and print nothing, whatever follows the
    /// name, and neither reads a player or a client flag.
    #[test]
    fn togglehelm_and_togglecloak_each_send_their_opcode_whatever_the_arguments() {
        let (mut app, rx) = commands_app();
        for line in ["togglehelm", "ToggleHelm on", "togglehelm 1 2 3"] {
            assert!(execute(app.world_mut(), line).is_empty(), "{line}");
            assert!(
                matches!(rx.try_recv(), Ok(ClientCommand::ToggleHelm)),
                "{line}"
            );
        }
        for line in ["togglecloak", "TOGGLECLOAK off"] {
            assert!(execute(app.world_mut(), line).is_empty(), "{line}");
            assert!(
                matches!(rx.try_recv(), Ok(ClientCommand::ToggleCloak)),
                "{line}"
            );
        }
        assert!(rx.try_recv().is_err(), "one send per line");
    }

    /// `0x404a2e`-`0x404a48`: category 4 (`game`), no help string.
    #[test]
    fn the_toggles_are_game_commands_with_no_help() {
        let (mut app, _rx) = commands_app();
        let game = execute(app.world_mut(), "help game");
        assert_eq!(game[0], "Commands registered for the category game:");
        for name in ["togglehelm", "togglecloak"] {
            assert!(
                game[1..].iter().any(|l| l.split(", ").any(|n| n == name)),
                "{name}: {game:?}"
            );
        }
        assert_eq!(
            execute(app.world_mut(), "help togglehelm"),
            [
                "Help for command togglehelm:",
                "     togglehelm No help yet"
            ]
        );
    }
}
