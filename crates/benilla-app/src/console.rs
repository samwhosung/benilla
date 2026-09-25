//! The console command registry: the reference's `ConsoleCommand` table
//! (`ConsoleCommandRegister` `0x63f9e0`, runtime `[0x63f880, 0x640c50)`). Subsystems register
//! from their plugins; `/console <line>` reaches [`execute`] through the chat drain.
//!
//! Each CVar is also a command under its own name (`0x63dde0`): a bare name prints
//! `CVar "%s" is "%s"`, a name with a value sets it, looked up after the registered commands.
//! The built-ins are `ConsoleVar.cpp`'s four (`set`, `cvar_reset`, `cvar_default`, `cvarlist`)
//! plus the registry's `help`.
//!
//! The reference's drop-down console screen is not built: output is a system line in chat.

use std::collections::BTreeMap;

use bevy::prelude::*;

use crate::cvars::{Cvars, SetOutcome};

/// A command's body: the world and the trimmed argument tail in, the lines to print out.
pub(crate) type ConsoleHandler = fn(&mut World, &str) -> Vec<String>;

#[derive(Clone, Copy)]
struct ConsoleCommand {
    /// The registered spelling, as `help` prints it.
    name: &'static str,
    help: &'static str,
    run: ConsoleHandler,
}

/// The registry: lowercased name to command, in name order.
#[derive(Resource, Default)]
pub(crate) struct ConsoleCommands {
    by_key: BTreeMap<String, ConsoleCommand>,
}

/// Registers a command from a plugin, the reference's `ConsoleCommandRegister`.
pub(crate) trait ConsoleCommandApp {
    fn console_command(
        &mut self,
        name: &'static str,
        help: &'static str,
        run: ConsoleHandler,
    ) -> &mut Self;
}

impl ConsoleCommandApp for App {
    fn console_command(
        &mut self,
        name: &'static str,
        help: &'static str,
        run: ConsoleHandler,
    ) -> &mut Self {
        let mut table = self.world_mut().get_resource_or_init::<ConsoleCommands>();
        let key = name.to_ascii_lowercase();
        // Two registrations of one name is a wiring bug.
        assert!(
            !table.by_key.contains_key(&key),
            "console command {name:?} registered twice"
        );
        table.by_key.insert(key, ConsoleCommand { name, help, run });
        self
    }
}

/// The built-ins: the reference's four `ConsoleVar.cpp` commands and `help`.
pub(crate) struct ConsolePlugin;

impl Plugin for ConsolePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ConsoleCommands>()
            .console_command("set", "Set the value of a CVar.", set)
            .console_command("cvar_reset", "Reset a CVar to its default.", cvar_default)
            .console_command("cvar_default", "Reset a CVar to its default.", cvar_default)
            .console_command(
                "cvarlist",
                "List the CVars, optionally matching a string.",
                cvarlist,
            )
            .console_command("help", "List the console commands, or describe one.", help);
    }
}

/// Runs one console line, the reference's `ConsoleCommandExecute`: a registered command, else a
/// CVar's own command, else unknown, matched case-insensitively. Returns the lines to print.
///
/// A CVar write runs its observers before this returns, as the callback runs inside `CVar::Set`
/// in the reference.
pub(crate) fn execute(world: &mut World, line: &str) -> Vec<String> {
    let line = line.trim();
    let (name, args) = line
        .split_once(char::is_whitespace)
        .map_or((line, ""), |(n, a)| (n, a.trim()));
    if name.is_empty() {
        return vec!["console: no command given (`help` lists the commands)".to_string()];
    }
    let command = world
        .get_resource::<ConsoleCommands>()
        .and_then(|t| t.by_key.get(&name.to_ascii_lowercase()).copied());
    if let Some(command) = command {
        return (command.run)(world, args);
    }
    if world
        .get_resource::<Cvars>()
        .is_some_and(|c| c.row(name).is_some())
    {
        return if args.is_empty() {
            vec![print_cvar(world, name)]
        } else {
            set_cvar(world, name, args)
        };
    }
    vec![format!(
        "console: '{name}' is not a command or a CVar (`help` lists the commands)"
    )]
}

/// `CVar "%s" is "%s"`, the per-CVar command with no argument (`0x63dde0`), plus any staged
/// value.
fn print_cvar(world: &World, name: &str) -> String {
    let cvars = world.resource::<Cvars>();
    let Some(row) = cvars.row(name) else {
        return format!("console: no CVar named '{name}'");
    };
    match &row.pending {
        Some(staged) => format!(
            "CVar \"{}\" is \"{}\" (\"{staged}\" staged until the next RestartGx)",
            row.name, row.value
        ),
        None => format!("CVar \"{}\" is \"{}\"", row.name, row.value),
    }
}

/// A write through the registry with its observers run, and the outcome as a line.
fn set_cvar(world: &mut World, name: &str, value: &str) -> Vec<String> {
    let (outcome, events) = {
        let mut cvars = world.resource_mut::<Cvars>();
        let outcome = cvars.set(name, value);
        (outcome, cvars.take_events())
    };
    for event in events {
        world.trigger(event);
    }
    match outcome {
        SetOutcome::Unknown => vec![format!("console: no CVar named '{name}'")],
        SetOutcome::Refused => vec![format!(
            "console: '{value}' is not a number, and {name} takes one"
        )],
        SetOutcome::Unchanged | SetOutcome::Changed => Vec::new(),
        SetOutcome::Staged => vec![format!(
            "{name} staged as \"{value}\" — applies at the next RestartGx"
        )],
    }
}

/// `set <name> <value>` (`0x63d500`). Deviation: the reference registers an unknown name as a new
/// category-5 record; this refuses it, because nothing would read the row and `config.toml`
/// keeps an unknown key anyway.
fn set(world: &mut World, args: &str) -> Vec<String> {
    let (name, value) = args
        .split_once(char::is_whitespace)
        .map_or((args, ""), |(n, v)| (n, v.trim()));
    if name.is_empty() || value.is_empty() {
        return vec!["usage: set <cvar> <value>".to_string()];
    }
    set_cvar(world, name, value)
}

/// `cvar_reset` and `cvar_default` (`0x63d590`, `0x63d640`): one body, since a row has no reset
/// value apart from its default.
fn cvar_default(world: &mut World, args: &str) -> Vec<String> {
    let name = args.split_whitespace().next().unwrap_or("");
    if name.is_empty() {
        return vec!["usage: cvar_default <cvar>".to_string()];
    }
    let default = world
        .resource::<Cvars>()
        .default_of(name)
        .map(str::to_string);
    let Some(default) = default else {
        return vec![format!("console: no CVar named '{name}'")];
    };
    set_cvar(world, name, &default)
}

/// `cvarlist [match]` (`0x63d6f0`), one line per row: value, default when moved, staged value,
/// and whether the session or an addon owns it. The reference's line format is untraced; this
/// one is benilla's.
fn cvarlist(world: &mut World, args: &str) -> Vec<String> {
    let wanted = args.split_whitespace().next().map(str::to_ascii_lowercase);
    let cvars = world.resource::<Cvars>();
    let mut rows: Vec<_> = cvars
        .rows()
        .filter(|r| {
            wanted
                .as_deref()
                .is_none_or(|w| r.name.to_ascii_lowercase().contains(w))
        })
        .collect();
    rows.sort_by_key(|r| r.name.to_ascii_lowercase());
    let mut out: Vec<String> = rows
        .iter()
        .map(|r| {
            let mut line = format!("{} = \"{}\"", r.name, r.value);
            if r.value != r.default {
                line.push_str(&format!(" (default \"{}\")", r.default));
            }
            if let Some(staged) = &r.pending {
                line.push_str(&format!(" [staged \"{staged}\"]"));
            }
            if cvars.is_session_owned(&r.name) {
                line.push_str(" [session]");
            }
            if r.addon {
                line.push_str(" [addon]");
            }
            line
        })
        .collect();
    out.push(format!("{} CVar(s)", rows.len()));
    out
}

/// `help [command]`: the registry's listing.
fn help(world: &mut World, args: &str) -> Vec<String> {
    let table = world.resource::<ConsoleCommands>();
    let wanted = args.split_whitespace().next();
    match wanted {
        Some(name) => match table.by_key.get(&name.to_ascii_lowercase()) {
            Some(c) => vec![format!("{} — {}", c.name, c.help)],
            None => vec![format!("console: no command named '{name}'")],
        },
        None => {
            let mut out: Vec<String> = table
                .by_key
                .values()
                .map(|c| format!("{} — {}", c.name, c.help))
                .collect();
            out.push(
                "…and every CVar by name: `<cvar>` prints it, `<cvar> <value>` sets it".into(),
            );
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console_app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .init_resource::<Cvars>()
            .add_plugins(ConsolePlugin);
        app
    }

    fn run(app: &mut App, line: &str) -> Vec<String> {
        execute(app.world_mut(), line)
    }

    #[test]
    fn a_line_is_a_command_then_a_cvar_then_unknown() {
        let mut app = console_app();
        assert_eq!(
            run(&mut app, "MusicVolume"),
            vec!["CVar \"MusicVolume\" is \"0.4\"".to_string()]
        );
        assert!(
            run(&mut app, "musicvolume 0.7").is_empty(),
            "a set is quiet"
        );
        assert_eq!(
            app.world().resource::<Cvars>().get("MusicVolume"),
            Some("0.7")
        );
        assert_eq!(
            run(&mut app, "SET MusicVolume 0.2"),
            Vec::<String>::new(),
            "`set` is a registered command, matched case-insensitively"
        );
        assert_eq!(
            app.world().resource::<Cvars>().get("MusicVolume"),
            Some("0.2")
        );
        assert_eq!(
            run(&mut app, "cvar_default musicVolume"),
            Vec::<String>::new()
        );
        assert_eq!(
            app.world().resource::<Cvars>().get("MusicVolume"),
            Some("0.4")
        );
        assert_eq!(
            run(&mut app, "nosuchthing 1"),
            vec![
                "console: 'nosuchthing' is not a command or a CVar (`help` lists the commands)"
                    .to_string()
            ]
        );
        assert_eq!(
            run(&mut app, "set nosuchthing 1"),
            vec!["console: no CVar named 'nosuchthing'".to_string()],
            "the stated divergence: no record conjured at the console"
        );
        assert_eq!(
            run(&mut app, "MusicVolume wat"),
            vec!["console: 'wat' is not a number, and MusicVolume takes one".to_string()]
        );
        assert_eq!(
            run(&mut app, "  "),
            vec!["console: no command given (`help` lists the commands)".to_string()]
        );
    }

    /// A write to a latched row says it is staged, prints as staged, and the list shows it.
    #[test]
    fn a_latched_cvar_reports_its_stage() {
        let mut app = console_app();
        assert_eq!(
            run(&mut app, "gxVSync 0"),
            vec!["gxVSync staged as \"0\" — applies at the next RestartGx".to_string()]
        );
        assert_eq!(
            run(&mut app, "gxvsync"),
            vec!["CVar \"gxVSync\" is \"1\" (\"0\" staged until the next RestartGx)".to_string()]
        );
        let list = run(&mut app, "cvarlist gxv");
        assert_eq!(
            list,
            vec![
                "gxVSync = \"1\" [staged \"0\"]".to_string(),
                "1 CVar(s)".to_string()
            ]
        );
    }

    #[test]
    fn cvarlist_says_where_each_row_stands() {
        let mut app = console_app();
        {
            let mut cvars = app.world_mut().resource_mut::<Cvars>();
            cvars.set("farclip", "500");
            cvars.own_for_session("uiScale", Some("1.2"));
            cvars.learn_addon_row("myAddonKnob", "7");
        }
        let list = run(&mut app, "cvarlist");
        assert!(
            list.contains(&"farclip = \"500\" (default \"350\")".to_string()),
            "{list:?}"
        );
        assert!(
            list.contains(&"uiScale = \"1.2\" (default \"0.9\") [session]".to_string()),
            "{list:?}"
        );
        assert!(
            list.contains(&"myAddonKnob = \"7\" [addon]".to_string()),
            "{list:?}"
        );
        assert!(list.last().unwrap().ends_with(" CVar(s)"));
        let some = run(&mut app, "cvarlist FARCLIP");
        assert_eq!(some.len(), 2, "matched case-insensitively: {some:?}");
    }

    #[test]
    fn help_lists_the_registry_and_a_registration_is_one_call() {
        fn shout(_: &mut World, args: &str) -> Vec<String> {
            vec![format!("{}!", args.to_ascii_uppercase())]
        }
        let mut app = console_app();
        app.console_command("shout", "Shout the argument back.", shout);
        assert_eq!(
            run(&mut app, "Shout hello there"),
            vec!["HELLO THERE!".to_string()]
        );
        let listing = run(&mut app, "help");
        assert!(listing.contains(&"shout — Shout the argument back.".to_string()));
        assert!(listing.contains(&"set — Set the value of a CVar.".to_string()));
        assert_eq!(
            run(&mut app, "help cvarlist"),
            vec!["cvarlist — List the CVars, optionally matching a string.".to_string()]
        );
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn a_name_registered_twice_is_a_wiring_bug() {
        fn nothing(_: &mut World, _: &str) -> Vec<String> {
            Vec::new()
        }
        let mut app = console_app();
        app.console_command("set", "again", nothing);
    }
}
