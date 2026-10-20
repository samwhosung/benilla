//! The console commands the reference registers with its player object: `RegisterPlayerCommands`
//! (`0x4049e0`), the first virtual of the object `ClientInitializeGame` builds (`0x401726`), runs
//! with no build or account test on the way, and `/console` reaches its commands through
//! `0x63ce00`, which has no category gate. `ClientDestroyGame` unregisters all six (slot 1,
//! `0x401f86` to `0x405390`) and `whois` too (`0x5ae316`), so the reference holds them from world
//! entry only. Registered for the process here they answer the same: the one route into `execute`
//! at the glue is `drain_glue_console`, and stock GlueXML never calls `ConsoleExec`. Each is a
//! send to the server; whether the account may use it is the server's to say.

use benilla_assets::coords::bevy_to_wow;
use benilla_assets::MapCatalogRes;
use benilla_ui::script::{s_str_to_float, s_str_tokenize};
use bevy::prelude::*;

use crate::console::{ConsoleCommandApp, DEBUG, GAME};
use crate::net::{ClientCommand, NetCommands};
use crate::player::Player;

pub(crate) struct PlayerCommandsPlugin;

impl Plugin for PlayerCommandsPlugin {
    fn build(&self, app: &mut App) {
        // Registered in the reference's order, none with a help string (`0x4049e0`-`0x404a48`):
        // category 0 for the three that move the player, 4 for the two toggles.
        app.console_command("port", DEBUG, None, port)
            .console_command("worldport", DEBUG, None, worldport)
            .console_command("setrawpos", DEBUG, None, setrawpos)
            .console_command("togglehelm", GAME, None, toggle_helm)
            .console_command("togglecloak", GAME, None, toggle_cloak);
    }
}

/// What `port` and `setrawpos` split their arguments at (`0x82ef90`): tab, CR, LF, `"`, space and
/// comma.
const POSITION_DELIMS: &[u8] = b"\t\r\n\" ,";

/// What `worldport` splits at (`0x82ecc8`): the same without the comma.
const WORLDPORT_DELIMS: &[u8] = b"\t\r\n\" ";

/// The buffer each argument is tokenized into (`push 0x40`, `0x404e5a`): 63 bytes of text.
const TOKEN_BYTES: usize = 0x40;

/// `ds:0x7ffaac`, pi / 180 as a float: what a facing argument in degrees is multiplied by.
const DEGREES_TO_RADIANS: f64 = 0.017_453_292_f32 as f64;

/// Queues `command` for the write thread, which drops it with no session as `NetClient::Send`
/// (`0x5379a0`) sends nothing unconnected. Nothing is printed.
fn send(world: &World, command: ClientCommand) -> Vec<String> {
    if let Some(net) = world.get_resource::<NetCommands>() {
        let _ = net.0.send(command);
    }
    Vec::new()
}

/// The next argument of `rest` (`SStrTokenize`, `0x64ae50`) in a [`TOKEN_BYTES`] buffer; empty at
/// the end of the line.
fn token(rest: &mut &[u8], delims: &[u8]) -> String {
    String::from_utf8_lossy(&s_str_tokenize(rest, TOKEN_BYTES, delims)).into_owned()
}

/// A numeric argument as the handlers store it: `SStrToFloat` (`0x64aaa0`) narrowed to a float.
fn float(text: &str) -> f32 {
    s_str_to_float(text) as f32
}

/// `0x404d10(x, y, 0, 0)`: x and y are finite and each lies in `(-H, H]`, H = 17066.666
/// (`0x7ffab4`, a float; `0x7ffab0` is 2H). The test is `0 <= H - v < 2H`, with `H - x` rounded to
/// a float before it and `H - y` kept at the stack's width (`0x404d6c`).
fn in_range(x: f32, y: f32) -> bool {
    const H: f32 = f32::from_bits(0x4685_5555);
    const TWICE_H: f32 = f32::from_bits(0x4705_5555);
    if !x.is_finite() || !y.is_finite() {
        return false;
    }
    let from_x = (f64::from(H) - f64::from(x)) as f32;
    let from_y = f64::from(H) - f64::from(y);
    (0.0..f64::from(TWICE_H)).contains(&from_y) && (0.0..TWICE_H).contains(&from_x)
}

/// Where the active player stands, in the wire's terms: the position in world yards and the
/// facing in radians in `[0, 2pi)`. `None` when no player is active, where the handlers do
/// nothing (`ClntObjMgrObjectPtr` is null, `0x404aab`, `0x404dea`, `0x405046`).
fn player_pose(world: &World) -> Option<([f32; 3], f32)> {
    let player = world.get_resource::<Player>().filter(|p| p.active)?;
    Some((
        bevy_to_wow(player.pos),
        player.facing().rem_euclid(std::f32::consts::TAU),
    ))
}

/// What `port` and `setrawpos` read after the first argument looks like a number: `x y [z]
/// [facing]`, with `z` 0 and the facing the player's own when absent. `None` is the handler's
/// silent return; `Err` is the line it prints.
fn read_position(args: &str, facing: f32) -> Option<Result<([f32; 3], f32), &'static str>> {
    let mut rest = args.as_bytes();
    let x = token(&mut rest, POSITION_DELIMS);
    if x.is_empty() {
        return None;
    }
    let y = token(&mut rest, POSITION_DELIMS);
    if y.is_empty() {
        return None;
    }
    let (x, y) = (float(&x), float(&y));
    if !in_range(x, y) {
        return Some(Err("Coordinates out of range\n"));
    }
    let z = token(&mut rest, POSITION_DELIMS);
    let z = if z.is_empty() { 0.0 } else { float(&z) };
    let degrees = token(&mut rest, POSITION_DELIMS);
    let facing = if degrees.is_empty() {
        facing
    } else {
        (s_str_to_float(&degrees) * DEGREES_TO_RADIANS) as f32
    };
    Some(Ok(([x, y, z], facing)))
}

/// Whether `args` opens a number: `IsAsciiDigit` (`0x6c9c30`) or `-` on its first byte
/// (`0x404ab8`, `0x404ac8`).
fn starts_a_number(args: &str) -> bool {
    matches!(args.as_bytes().first(), Some(b'0'..=b'9' | b'-'))
}

/// `PortCommand` (`0x404a60`) with a player: a number-first line is `x y [z] [facing]`, checked by
/// `0x404d10`, and sent as `MSG_MOVE_TELEPORT_CHEAT` (`0x404cdc`); any other text is a player
/// name, sent whole as `CMSG_TELEPORT_TO_UNIT` (`0x404b31`), an empty line too.
fn port(world: &mut World, args: &str) -> Vec<String> {
    let Some((_, facing)) = player_pose(world) else {
        return Vec::new();
    };
    if !starts_a_number(args) {
        return send(
            world,
            ClientCommand::TeleportToUnit {
                text: args.to_string(),
            },
        );
    }
    match read_position(args, facing) {
        None => Vec::new(),
        Some(Err(line)) => vec![line.to_string()],
        Some(Ok((pos, facing))) => send(world, ClientCommand::TeleportCheat { pos, facing }),
    }
}

/// `SetRawPosCommand` (`0x405000`) with a player: `port`'s numbers, sent as
/// `CMSG_MOVE_SET_RAW_POSITION` (`0x4051e4`); other text sends nothing.
fn setrawpos(world: &mut World, args: &str) -> Vec<String> {
    let Some((_, facing)) = player_pose(world) else {
        return Vec::new();
    };
    if !starts_a_number(args) {
        return Vec::new();
    }
    match read_position(args, facing) {
        None => Vec::new(),
        Some(Err(line)) => vec![line.to_string()],
        Some(Ok((pos, facing))) => send(world, ClientCommand::SetRawPosition { pos, facing }),
    }
}

/// `SStrToUnsigned` (`0x64ade0`): decimal digits in a wrapping 32-bit register, and 0 for a
/// string that does not open with a digit.
fn s_str_to_unsigned(text: &str) -> u32 {
    let digits = text
        .bytes()
        .map_while(|b| b.checked_sub(b'0').filter(|&d| d < 10));
    digits.fold(0u32, |n, d| n.wrapping_mul(10).wrapping_add(u32::from(d)))
}

/// `worldport` (`0x404dc0`) with a player: `<map> [x y z] [facing]`, each of the four defaulting
/// to where the player stands. No map prints the usage (`0x82efb0`); a map with no `Map.dbc` row
/// prints `Bad world number` (`0x82ef98`). Sent as `CMSG_WORLD_TELEPORT` (`0x404fc8`) with the
/// millisecond clock (`0x42c010`). No range test is made on the position.
fn worldport(world: &mut World, args: &str) -> Vec<String> {
    let Some((mut pos, mut facing)) = player_pose(world) else {
        return Vec::new();
    };
    let mut rest = args.as_bytes();
    let map = token(&mut rest, WORLDPORT_DELIMS);
    if map.is_empty() {
        return vec!["Usage: worldport <continentID> [x y z] [facing]".to_string()];
    }
    let map = s_str_to_unsigned(&map);
    for axis in &mut pos {
        let text = token(&mut rest, WORLDPORT_DELIMS);
        if !text.is_empty() {
            *axis = float(&text);
        }
    }
    let degrees = token(&mut rest, WORLDPORT_DELIMS);
    if !degrees.is_empty() {
        facing = (s_str_to_float(&degrees) * DEGREES_TO_RADIANS) as f32;
    }
    // `0 <= map <= [0xc0daac]` and a row in the table at `[0xc0daa8]` (`0x404eef`-`0x404f04`).
    let known = i32::try_from(map).is_ok()
        && world
            .get_resource::<MapCatalogRes>()
            .is_some_and(|maps| maps.0.directory(map).is_some());
    if !known {
        return vec![format!("Bad world number: {}\n", map as i32)];
    }
    let time_ms = world
        .get_resource::<Time<Real>>()
        .map_or(0, crate::ui_billing::now_ms);
    send(
        world,
        ClientCommand::WorldTeleport {
            time_ms,
            map,
            pos,
            facing,
        },
    )
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
    use benilla_assets::coords::wow_to_bevy;
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

    /// The same, with an active player at world (100, 200, 30) facing 1 radian, and a `Map.dbc`
    /// of maps 0 and 1.
    fn in_world() -> (App, Receiver<ClientCommand>) {
        let (mut app, rx) = commands_app();
        let mut player = Player::default();
        player.active = true;
        player.pos = wow_to_bevy([100.0, 200.0, 30.0]);
        player.turn_aim(1.0);
        app.insert_resource(player).insert_resource(MapCatalogRes(
            benilla_formats::MapCatalog::from_rows([(0, "Azeroth", 0), (1, "Kalimdor", 0)]),
        ));
        (app, rx)
    }

    fn run(app: &mut App, line: &str) -> Vec<String> {
        execute(app.world_mut(), line)
    }

    /// `0x405250` and `0x4052f0` each send one opcode and print nothing, whatever follows the
    /// name, and neither reads a player or a client flag.
    #[test]
    fn togglehelm_and_togglecloak_each_send_their_opcode_whatever_the_arguments() {
        let (mut app, rx) = commands_app();
        for line in ["togglehelm", "ToggleHelm on", "togglehelm 1 2 3"] {
            assert!(run(&mut app, line).is_empty(), "{line}");
            assert!(
                matches!(rx.try_recv(), Ok(ClientCommand::ToggleHelm)),
                "{line}"
            );
        }
        for line in ["togglecloak", "TOGGLECLOAK off"] {
            assert!(run(&mut app, line).is_empty(), "{line}");
            assert!(
                matches!(rx.try_recv(), Ok(ClientCommand::ToggleCloak)),
                "{line}"
            );
        }
        assert!(rx.try_recv().is_err(), "one send per line");
    }

    /// `0x4049e0`-`0x404a48`: the three position commands in category 0 (`debug`), the toggles in
    /// 4 (`game`), none with a help string.
    #[test]
    fn the_commands_register_in_their_categories_with_no_help() {
        let (mut app, _rx) = commands_app();
        for (category, names) in [
            ("debug", &["port", "worldport", "setrawpos"][..]),
            ("game", &["togglehelm", "togglecloak"][..]),
        ] {
            let listed = run(&mut app, &format!("help {category}"));
            for name in names {
                assert!(
                    listed[1..]
                        .iter()
                        .any(|l| l.split(", ").any(|n| n == *name)),
                    "{name}: {listed:?}"
                );
            }
        }
        assert_eq!(
            run(&mut app, "help togglehelm"),
            [
                "Help for command togglehelm:",
                "     togglehelm No help yet"
            ]
        );
    }

    /// `0x404ab8`-`0x404b31`: a line that does not open with a digit or `-` is a player name, sent
    /// whole (an empty one too) as `CMSG_TELEPORT_TO_UNIT`.
    #[test]
    fn port_sends_other_text_as_a_unit_teleport() {
        let (mut app, rx) = in_world();
        for (line, text) in [
            ("port Thrall", "Thrall"),
            ("port", ""),
            ("port Thrall Jaina", "Thrall Jaina"),
            ("port x1 2", "x1 2"),
        ] {
            assert!(run(&mut app, line).is_empty(), "{line}");
            match rx.try_recv() {
                Ok(ClientCommand::TeleportToUnit { text: sent }) => assert_eq!(sent, text),
                other => panic!("{line}: {other:?}"),
            }
        }
    }

    /// `0x404b66`-`0x404cdc`: `x y [z] [facing]` split at spaces, tabs, quotes and commas, `z` 0 and
    /// the facing the player's own when absent, a given facing in degrees.
    #[test]
    fn port_sends_a_cheat_teleport_for_numbers() {
        let (mut app, rx) = in_world();
        let degrees = |d: f64| (d * f64::from(0.017_453_292_f32)) as f32;
        let sent = |app: &mut App, line: &str| {
            assert!(run(app, line).is_empty(), "{line}");
            match rx.try_recv() {
                Ok(ClientCommand::TeleportCheat { pos, facing }) => (pos, facing),
                other => panic!("{line}: {other:?}"),
            }
        };
        assert_eq!(sent(&mut app, "port 10 20"), ([10.0, 20.0, 0.0], 1.0));
        assert_eq!(sent(&mut app, "port -10.5,20 3"), ([-10.5, 20.0, 3.0], 1.0));
        assert_eq!(
            sent(&mut app, "port 1 2 3 90"),
            ([1.0, 2.0, 3.0], degrees(90.0))
        );
        assert_eq!(
            sent(&mut app, "port 5 6 7 abc"),
            ([5.0, 6.0, 7.0], 0.0),
            "a word reads as 0 degrees"
        );
    }

    /// `0x404b7d`, `0x404bac`: no `x` or no `y` returns before anything is sent or said.
    #[test]
    fn port_with_a_lone_number_sends_and_says_nothing() {
        let (mut app, rx) = in_world();
        for line in ["port 5", "port -", "port 5 ,", "port 5 \"\""] {
            assert!(run(&mut app, line).is_empty(), "{line}");
        }
        assert!(rx.try_recv().is_err());
    }

    /// `0x404d10`: x and y each in `(-17066.666, 17066.666]`, finite; the line is
    /// `Coordinates out of range` (`0x82ef74`) and nothing is sent.
    #[test]
    fn a_position_off_the_map_square_is_refused_with_a_line() {
        let h = f32::from_bits(0x4685_5555);
        assert!(in_range(0.0, 0.0));
        assert!(in_range(h, h), "H itself is inside");
        assert!(in_range(-h + 0.01, -h + 0.01));
        assert!(!in_range(-h, 0.0), "H - x reaches 2H");
        assert!(!in_range(0.0, -h));
        assert!(!in_range(h + 0.01, 0.0));
        assert!(!in_range(0.0, h + 0.01));
        assert!(!in_range(f32::NAN, 0.0));
        assert!(!in_range(0.0, f32::INFINITY));
        let (mut app, rx) = in_world();
        for line in [
            "port 20000 0",
            "port 0 -17066.7",
            "port 1e50 0",
            "setrawpos 17067 0",
        ] {
            assert_eq!(
                run(&mut app, line),
                ["Coordinates out of range\n"],
                "{line}"
            );
        }
        assert!(rx.try_recv().is_err());
    }

    /// `0x40505e`-`0x4051e4`: `setrawpos` reads and checks the same numbers and sends
    /// `CMSG_MOVE_SET_RAW_POSITION`; other text returns with nothing sent.
    #[test]
    fn setrawpos_sends_the_numbers_and_ignores_text() {
        let (mut app, rx) = in_world();
        assert!(run(&mut app, "setrawpos Thrall").is_empty());
        assert!(run(&mut app, "setrawpos").is_empty());
        assert!(rx.try_recv().is_err());
        run(&mut app, "setrawpos 10 20 30 180");
        match rx.try_recv() {
            Ok(ClientCommand::SetRawPosition { pos, facing }) => {
                assert_eq!(pos, [10.0, 20.0, 30.0]);
                assert_eq!(facing, (180.0 * f64::from(0.017_453_292_f32)) as f32);
            }
            other => panic!("{other:?}"),
        }
    }

    /// `ClntObjMgrObjectPtr` is null with no active player (`0x404aab`, `0x404dea`, `0x405046`):
    /// the three return at once.
    #[test]
    fn the_position_commands_do_nothing_without_an_active_player() {
        let (mut app, rx) = commands_app();
        app.insert_resource(Player::default());
        for line in [
            "port Thrall",
            "port 1 2",
            "setrawpos 1 2",
            "worldport 1",
            "worldport",
        ] {
            assert!(run(&mut app, line).is_empty(), "{line}");
        }
        assert!(rx.try_recv().is_err());
    }

    /// `0x404dc0`: no map prints the usage (`0x82efb0`), a map with no row `Bad world number`
    /// (`0x82ef98`, `%i` of the signed value), neither sends.
    #[test]
    fn worldport_refuses_a_missing_or_unknown_map() {
        let (mut app, rx) = in_world();
        assert_eq!(
            run(&mut app, "worldport"),
            ["Usage: worldport <continentID> [x y z] [facing]"]
        );
        for (line, said) in [
            ("worldport 2", "Bad world number: 2\n"),
            ("worldport 4294967295", "Bad world number: -1\n"),
            ("worldport 99999 1 2 3", "Bad world number: 99999\n"),
        ] {
            assert_eq!(run(&mut app, line), [said], "{line}");
        }
        assert!(rx.try_recv().is_err());
    }

    /// `0x404f1e`-`0x404fc8`: the clock, the map and the four numbers, each defaulting to the
    /// player's own; no range test, and a comma is not a delimiter here (`0x82ecc8`).
    #[test]
    fn worldport_sends_the_map_and_the_position() {
        let (mut app, rx) = in_world();
        app.insert_resource({
            let mut time = Time::<Real>::default();
            time.advance_by(std::time::Duration::from_millis(1234));
            time
        });
        let sent = |app: &mut App, line: &str| {
            assert!(run(app, line).is_empty(), "{line}");
            match rx.try_recv() {
                Ok(ClientCommand::WorldTeleport {
                    time_ms,
                    map,
                    pos,
                    facing,
                }) => (time_ms, map, pos, facing),
                other => panic!("{line}: {other:?}"),
            }
        };
        let (time_ms, map, pos, facing) = sent(&mut app, "worldport 1 16226 16257 13.2 90");
        assert_eq!((time_ms, map), (1234, 1));
        assert_eq!(pos, [16226.0, 16257.0, 13.2]);
        assert_eq!(facing, (90.0 * f64::from(0.017_453_292_f32)) as f32);
        let (_, map, pos, facing) = sent(&mut app, "worldport 0");
        assert_eq!((map, pos, facing), (0, [100.0, 200.0, 30.0], 1.0));
        assert_eq!(
            sent(&mut app, "worldport 1 50000 60000 70000").2,
            [50000.0, 60000.0, 70000.0],
            "no range test"
        );
        assert_eq!(
            sent(&mut app, "worldport 1 5 6").2,
            [5.0, 6.0, 30.0],
            "a missing z"
        );
        let (_, map, pos, _) = sent(&mut app, "worldport 1,5,6");
        assert_eq!(
            (map, pos),
            (1, [100.0, 200.0, 30.0]),
            "`1,5,6` is the map token"
        );
        assert_eq!(
            sent(&mut app, "worldport abc").1,
            0,
            "SStrToUnsigned reads a non-digit as 0"
        );
    }

    /// `SStrToUnsigned` (`0x64ade0`): digits in a wrapping dword, 0 from a first non-digit.
    #[test]
    fn s_str_to_unsigned_reads_digits_and_wraps() {
        for (text, want) in [
            ("12", 12),
            ("007", 7),
            ("1x9", 1),
            ("x1", 0),
            ("-1", 0),
            ("4294967295", u32::MAX),
            ("4294967297", 1),
        ] {
            assert_eq!(s_str_to_unsigned(text), want, "{text}");
        }
    }
}
