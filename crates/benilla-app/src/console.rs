//! The console command registry: the reference's `ConsoleCommand` table
//! (`ConsoleCommandRegister` `0x63f9e0`, runtime `[0x63f880, 0x640c50)`). Subsystems register
//! from their plugins. A line from Lua (`ConsoleExec`, and so the typed `/console`) runs its CVar
//! writes on the VM's mirror inside the call, as the reference's runs inside `ConsoleExec`
//! (`0x515f43`); the rest reaches [`execute`] through the chat drain, which reads it with the same
//! split ([`benilla_ui::script::console_word`]).
//!
//! Each CVar is also a command under its own name (`0x63dde0`): a bare name prints
//! `CVar "%s" is "%s"` here, looked up after the registered commands; with a value it was a write,
//! which `ConsoleExec` ran. The built-ins here are `ConsoleVar.cpp`'s `cvarlist` and the registry's
//! `help`, which also lists the commands `ConsoleExec` runs (`CVAR_COMMANDS`).
//!
//! What a command prints goes to the log ([`run`]): the reference's `ConsoleWrite` (`0x63cb50`)
//! only links a line into the console screen's list, which opens only with `-console`
//! (`0x63c49b`), and fires no event, so in play nobody sees it.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::cvars::Cvars;

/// A command's body: the world and the argument tail in, the lines to print out.
pub(crate) type ConsoleHandler = fn(&mut World, &str) -> Vec<String>;

/// The nine command categories `help` names (`0x8653a0`, an id then a name per 0x18 bytes), in
/// id order: the index is the category a registration passes (`ConsoleCommandRegister`'s
/// `+0x20`). An addon's CVar registers with 9 (`0x488b81`), which names none.
const CATEGORIES: [&str; 9] = [
    "debug", "graphics", "console", "combat", "game", "default", "net", "sound", "gm",
];

/// A category id ([`CATEGORIES`]).
pub(crate) type ConsoleCategory = u8;

/// `graphics`.
pub(crate) const GRAPHICS: ConsoleCategory = 1;
/// `console`.
const CONSOLE: ConsoleCategory = 2;
/// `default`.
const DEFAULT: ConsoleCategory = 5;

#[derive(Clone, Copy)]
struct ConsoleCommand {
    /// The registered spelling, as `help` prints it.
    name: &'static str,
    category: ConsoleCategory,
    /// `None` for a registration that passes no help (`+0x1c` zero), which `help` answers with
    /// `No help yet` (`0x64008f`).
    help: Option<&'static str>,
    run: ConsoleHandler,
}

/// The registry: commands in registration order, and lowercased name to index.
#[derive(Resource, Default)]
pub(crate) struct ConsoleCommands {
    commands: Vec<ConsoleCommand>,
    by_key: HashMap<String, usize>,
}

impl ConsoleCommands {
    fn get(&self, name: &str) -> Option<ConsoleCommand> {
        self.by_key
            .get(&name.to_ascii_lowercase())
            .map(|&i| self.commands[i])
    }
}

/// Registers a command from a plugin, the reference's `ConsoleCommandRegister`.
pub(crate) trait ConsoleCommandApp {
    fn console_command(
        &mut self,
        name: &'static str,
        category: ConsoleCategory,
        help: Option<&'static str>,
        run: ConsoleHandler,
    ) -> &mut Self;
}

impl ConsoleCommandApp for App {
    fn console_command(
        &mut self,
        name: &'static str,
        category: ConsoleCategory,
        help: Option<&'static str>,
        run: ConsoleHandler,
    ) -> &mut Self {
        let mut table = self.world_mut().get_resource_or_init::<ConsoleCommands>();
        let key = name.to_ascii_lowercase();
        // Two registrations of one name is a wiring bug.
        assert!(
            !table.by_key.contains_key(&key),
            "console command {name:?} registered twice"
        );
        let at = table.commands.len();
        table.commands.push(ConsoleCommand {
            name,
            category,
            help,
            run,
        });
        table.by_key.insert(key, at);
        self
    }
}

/// `ConsoleVar.cpp`'s commands that write a CVar, which `ConsoleExec` runs on the VM's mirror
/// inside the call (`benilla_ui::script`'s `console_exec`): listed by `help`, never run here.
/// Registered in category 5 with these helps (`0x63d3d0`-`0x63d40d`).
const CVAR_COMMANDS: [(&str, &str); 3] = [
    ("set", "Set the value of a CVar"),
    (
        "cvar_reset",
        "Set the value of a CVar to it's startup value",
    ),
    (
        "cvar_default",
        "Set the value of a CVar to it's coded default value",
    ),
];

/// The built-ins: `ConsoleVar.cpp`'s `cvarlist` (`0x63d423`) and `help` (`0x6400f1`).
pub(crate) struct ConsolePlugin;

impl Plugin for ConsolePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ConsoleCommands>()
            .console_command("cvarlist", DEFAULT, Some("List cvars"), cvarlist)
            .console_command(
                "help",
                CONSOLE,
                Some("Provides help information about a command."),
                help,
            );
    }
}

/// Run one console line `ConsoleExec` handed on, its replies to the log.
pub(crate) fn run(world: &mut World, line: &str) {
    for text in execute(world, line) {
        info!("console: {text}");
    }
}

/// Runs one console line `ConsoleExec` handed on, the reference's `ConsoleCommandExecute`
/// (`0x63ce00`): a registered command, else a CVar's name, matched case-insensitively, else
/// `Unknown command` (`0x865104`, `0x63cf2e`), an empty line too. Returns the lines to print.
pub(crate) fn execute(world: &mut World, line: &str) -> Vec<String> {
    let (name, args) = benilla_ui::script::console_word(line);
    let command = world
        .get_resource::<ConsoleCommands>()
        .and_then(|t| t.get(name));
    if let Some(command) = command {
        return (command.run)(world, args);
    }
    if let Some(row) = world.get_resource::<Cvars>().and_then(|c| c.row(name)) {
        // `0x63dde0` with no value (`ConsoleExec` ran every line that had one): `CVar "%s" is
        // "%s"` (`0x8652e0`) of the name as typed, the parse buffer in `ecx` (`0x63ce97`), and
        // the applied value, through `ConsolePrintf`'s 1024 bytes (`0x63cd50`).
        let mut line = format!("CVar \"{name}\" is \"{}\"", row.value);
        cut_to_bytes(&mut line, 0x3ff);
        return vec![line];
    }
    vec!["Unknown command".to_string()]
}

/// `cvarlist` (`0x63d6f0`), which reads no argument: one line per row in list order
/// ([`Cvars::listed`]), `  "%s" is "%s"` (`0x8652a8`) of the name and the applied value, then
/// ` (default "%s")` (`0x865298`) and ` (reset "%s")` (`0x865288`), each when the slot holds text
/// that differs from the value case-sensitively (`SStrCmp`, `0x63d753`, `0x63d7a2`). The line is
/// built in 0x100 bytes, so it stops at 255.
fn cvarlist(world: &mut World, _args: &str) -> Vec<String> {
    world
        .resource::<Cvars>()
        .listed()
        .into_iter()
        .map(|r| {
            let mut line = format!("  \"{}\" is \"{}\"", r.name, r.value);
            if r.default != r.value {
                line.push_str(&format!(" (default \"{}\")", r.default));
            }
            if let Some(reset) = r.reset.as_ref().filter(|reset| **reset != r.value) {
                line.push_str(&format!(" (reset \"{reset}\")"));
            }
            cut_to_bytes(&mut line, 0xff);
            line
        })
        .collect()
}

/// Cut `line` to at most `max` bytes, on a character boundary.
fn cut_to_bytes(line: &mut String, max: usize) {
    if line.len() > max {
        let mut end = max;
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
    }
}

/// The longest line `help` builds: its 0x80-byte buffers (`0x63fdc3`), and 0xc5 bytes for a
/// command's two lines (`0x64006a`).
const HELP_LINE: usize = 0x7f;
const HELP_COMMAND_LINE: usize = 0xc4;

/// `help` (`0x63fdc0`), which reads the whole argument tail as one word.
///
/// - None: `Console help categories: ` (`0x865560`), the nine names joined by `, ` (`0x84480c`)
///   and `For more information type 'help [command] or [category]'` (`0x865524`).
/// - A category, case-blind (`0x63fe87`): `Commands registered for the category %s:`
///   (`0x8654f8`), then its commands' names eight to a line joined by `, `, or `NONE` (`0x8421a0`)
///   for none. The command lookup below then runs as well (`0x640048`).
/// - A command, a CVar included, as each registers one (`0x63ddc9`): `Help for command %s:`
///   (`0x8654e0`) and `     %s %s` (`0x8654d4`) of the typed word and its help, else `No help
///   yet` (`0x865480`).
/// - Anything else prints nothing.
///
/// Deviation: a line of eight names is not left with its trailing `, ` and a list of a multiple
/// of eight names is not followed by `NONE`, two slips of the reference's loop (`0x63ffed`,
/// `0x64004d`). The commands list in registration order, then the CVars in the registry's list
/// order, where the reference's one table interleaves them as start-up registers them.
///
/// Deviation: a CVar lists under its registration's category with its registration's help, even
/// when `config.toml` held it at boot. In the reference such a record was made by the config
/// line's `set` (`0x63d569`-`0x63d582`), which registered its command in category 5 with the help
/// `""` (`0x63ddc9`), and the later registration finds the record and returns without registering
/// again (`0x63dc1a`-`0x63dc87`), so what `help` says would hang on what the file held.
fn help(world: &mut World, args: &str) -> Vec<String> {
    if args.is_empty() {
        return vec![
            "Console help categories: ".to_string(),
            CATEGORIES.join(", "),
            "For more information type 'help [command] or [category]'".to_string(),
        ];
    }
    let table = world.resource::<ConsoleCommands>();
    let cvars = world.resource::<Cvars>();
    let mut out = Vec::new();
    if let Some(category) = CATEGORIES.iter().position(|c| c.eq_ignore_ascii_case(args)) {
        let category = category as ConsoleCategory;
        let mut header = format!("Commands registered for the category {args}:");
        cut_to_bytes(&mut header, HELP_LINE);
        out.push(header);
        let commands = table
            .commands
            .iter()
            .filter(|c| c.category == category)
            .map(|c| c.name)
            .chain(
                CVAR_COMMANDS
                    .iter()
                    .filter(|_| category == DEFAULT)
                    .map(|(name, _)| *name),
            );
        let rows = cvars
            .listed()
            .into_iter()
            .filter(|r| crate::cvars::reference_help(&r.name).0 == category)
            .map(|r| r.name.as_str());
        let names: Vec<&str> = commands.chain(rows).collect();
        if names.is_empty() {
            out.push("NONE".to_string());
        }
        for eight in names.chunks(8) {
            let mut line = eight.join(", ");
            cut_to_bytes(&mut line, HELP_LINE);
            out.push(line);
        }
    }
    let help = if let Some(command) = table.get(args) {
        Some(command.help)
    } else if let Some((_, help)) = CVAR_COMMANDS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(args))
    {
        Some(Some(*help))
    } else {
        cvars
            .row(args)
            .map(|r| crate::cvars::reference_help(&r.name).1)
    };
    if let Some(help) = help {
        let mut title = format!("Help for command {args}:");
        let mut body = format!("     {args} {}", help.unwrap_or("No help yet"));
        cut_to_bytes(&mut title, HELP_COMMAND_LINE);
        cut_to_bytes(&mut body, HELP_COMMAND_LINE);
        out.extend([title, body]);
    }
    out
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

    /// `0x63ce00`: a registered command, else a CVar's own command, which prints the name as
    /// typed (`0x63ce97`) and the applied value (`0x63de08`), else `Unknown command`
    /// (`0x63cf2e`), an empty line included.
    #[test]
    fn a_line_is_a_command_then_a_cvar_then_unknown() {
        let mut app = console_app();
        assert_eq!(
            run(&mut app, "MusicVolume"),
            ["CVar \"MusicVolume\" is \"0.4\""]
        );
        assert_eq!(
            run(&mut app, "  musicvolume"),
            ["CVar \"musicvolume\" is \"0.4\""],
            "the typed spelling"
        );
        assert_eq!(run(&mut app, "nosuchthing 1"), ["Unknown command"]);
        assert_eq!(run(&mut app, "  "), ["Unknown command"]);
        assert_eq!(run(&mut app, ""), ["Unknown command"]);
    }

    /// A latched row prints its applied value, `rec+0x20`, not its stage.
    #[test]
    fn a_latched_cvar_prints_its_applied_value() {
        let mut app = console_app();
        app.world_mut().resource_mut::<Cvars>().set("gxVSync", "0");
        assert_eq!(run(&mut app, "gxvsync"), ["CVar \"gxvsync\" is \"1\""]);
    }

    /// `0x63d6f0`: every row whatever the argument, `  "%s" is "%s"`, then the default and the
    /// reset text where each differs from the value case-sensitively, in the table's order.
    #[test]
    fn cvarlist_prints_the_references_lines_in_list_order() {
        let mut app = console_app();
        app.world_mut()
            .insert_resource(Cvars::with_value("farclip", "400"));
        let line = |app: &mut App, name: &str| {
            run(app, "cvarlist")
                .into_iter()
                .find(|l| l.starts_with(&format!("  \"{name}\" ")))
                .unwrap()
        };
        assert_eq!(
            line(&mut app, "farclip"),
            "  \"farclip\" is \"400\" (default \"350\")",
            "the reset text is the value"
        );
        app.world_mut()
            .resource_mut::<Cvars>()
            .set("farclip", "500");
        assert_eq!(
            line(&mut app, "farclip"),
            "  \"farclip\" is \"500\" (default \"350\") (reset \"400\")"
        );
        app.world_mut()
            .resource_mut::<Cvars>()
            .set("farclip", "350");
        assert_eq!(
            line(&mut app, "farclip"),
            "  \"farclip\" is \"350\" (reset \"400\")"
        );
        assert_eq!(
            line(&mut app, "MusicVolume"),
            "  \"MusicVolume\" is \"0.4\""
        );
        // `SStrCmp`, not the save's case-blind compare.
        app.world_mut()
            .insert_resource(Cvars::with_value("ErrorFilter", "ALL"));
        assert_eq!(
            line(&mut app, "ErrorFilter"),
            "  \"ErrorFilter\" is \"ALL\" (default \"all\")"
        );
        app.world_mut()
            .resource_mut::<Cvars>()
            .learn_addon_row("myAddonKnob", "7");
        let all = run(&mut app, "cvarlist FARCLIP");
        let cvars = app.world().resource::<Cvars>();
        assert_eq!(all.len(), cvars.rows().count(), "the argument is not read");
        assert_eq!(all.last().unwrap(), "  \"myAddonKnob\" is \"7\"", "last");
        // The printed order is the table's, not the registry's registration order.
        let printed: Vec<&str> = all
            .iter()
            .map(|l| l.split('"').nth(1).expect("a quoted name"))
            .collect();
        let ranks: Vec<usize> = printed.iter().map(|n| crate::cvars::list_rank(n)).collect();
        assert!(ranks.is_sorted(), "the table's order: {printed:?}");
        assert!(ranks[0] < usize::MAX);
        let registered: Vec<&str> = cvars.rows().map(|r| r.name.as_str()).collect();
        assert_ne!(printed, registered, "sorted, not as registered");
    }

    /// The buffer is 0x100 bytes (`0x63d734`), so a line stops at 255.
    #[test]
    fn a_cvarlist_line_stops_at_255_bytes() {
        let mut app = console_app();
        let long = "x".repeat(300);
        app.world_mut()
            .resource_mut::<Cvars>()
            .set("realmName", &long);
        let line = run(&mut app, "cvarlist")
            .into_iter()
            .find(|l| l.starts_with("  \"realmName\" "))
            .unwrap();
        let mut want = format!("  \"realmName\" is \"{long}\"");
        want.truncate(255);
        assert_eq!(line, want);
    }

    /// `help` (`0x63fdc0`): the category listing with no argument, a category's commands and a
    /// command's help, a CVar's from its registration, each with the word as typed.
    #[test]
    fn help_prints_the_categories_a_category_and_a_command() {
        fn shout(_: &mut World, args: &str) -> Vec<String> {
            vec![format!("{}!", args.to_ascii_uppercase())]
        }
        let mut app = console_app();
        app.console_command("shout", GRAPHICS, None, shout);
        assert_eq!(run(&mut app, "Shout hello there"), ["HELLO THERE!"]);
        assert_eq!(
            run(&mut app, "help"),
            [
                "Console help categories: ",
                "debug, graphics, console, combat, game, default, net, sound, gm",
                "For more information type 'help [command] or [category]'",
            ]
        );
        assert_eq!(
            run(&mut app, "help CONSOLE"),
            ["Commands registered for the category CONSOLE:", "help"]
        );
        assert_eq!(
            run(&mut app, "help gm"),
            ["Commands registered for the category gm:", "NONE"]
        );
        let graphics = run(&mut app, "help graphics");
        assert_eq!(
            graphics[0],
            "Commands registered for the category graphics:"
        );
        assert!(graphics[1].starts_with("shout, "), "{graphics:?}");
        assert!(
            graphics[1..]
                .iter()
                .any(|l| l.split(", ").any(|n| n == "farclip")),
            "a CVar lists under its category"
        );
        assert!(graphics[1..graphics.len() - 1]
            .iter()
            .all(|l| l.split(", ").count() == 8));
        assert_eq!(
            run(&mut app, "help CVAR_RESET"),
            [
                "Help for command CVAR_RESET:",
                "     CVAR_RESET Set the value of a CVar to it's startup value",
            ]
        );
        assert_eq!(
            run(&mut app, "help shout"),
            ["Help for command shout:", "     shout No help yet"]
        );
        assert_eq!(
            run(&mut app, "help farclip"),
            [
                "Help for command farclip:",
                "     farclip Far clip plane distance",
            ]
        );
        assert!(run(&mut app, "help nosuch").is_empty());
        assert!(
            run(&mut app, "help set ").is_empty(),
            "the whole tail is the word"
        );
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn a_name_registered_twice_is_a_wiring_bug() {
        fn nothing(_: &mut World, _: &str) -> Vec<String> {
            Vec::new()
        }
        let mut app = console_app();
        app.console_command("cvarlist", DEFAULT, None, nothing);
    }
}
