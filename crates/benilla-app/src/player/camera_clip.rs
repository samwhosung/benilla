//! The `cameraClip` console command: the camera registers it and nothing reads what it stores.

use bevy::prelude::*;

use crate::console::{ConsoleCommandApp, DEBUG};

/// The word `cameraClip` sets or flips (`[0x84f78c]`, 1 at load; the stores `0x50ddcf` and
/// `0x50dde7` are its only references).
#[derive(Resource)]
pub(super) struct CameraClipWord(i32);

impl Default for CameraClipWord {
    fn default() -> Self {
        Self(1)
    }
}

/// `CameraClipCommand` (`0x50ddc0`): an argument is read by `SStrToInt` (`0x64ac60`) into the word,
/// none flips it between 0 and 1 (`0x50ddda`-`0x50dde7`). It prints nothing and returns 1.
fn camera_clip(world: &mut World, args: &str) -> Vec<String> {
    let mut word = world.resource_mut::<CameraClipWord>();
    word.0 = if args.is_empty() {
        i32::from(word.0 == 0)
    } else {
        benilla_ui::script::s_str_to_int(args)
    };
    Vec::new()
}

/// The camera's constructor registers the command in category 0 with no help (`0x50c3fb`).
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<CameraClipWord>()
        .console_command("cameraClip", DEBUG, None, camera_clip);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::{execute, ConsolePlugin};
    use crate::cvars::Cvars;

    fn camera_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Cvars>()
            .add_plugins(ConsolePlugin);
        plugin(&mut app);
        app
    }

    fn word(app: &App) -> i32 {
        app.world().resource::<CameraClipWord>().0
    }

    /// `0x50ddc0`: a number is stored, none flips 0 and 1, and nothing is printed either way.
    #[test]
    fn camera_clip_stores_a_number_or_flips_the_word_and_prints_nothing() {
        let mut app = camera_app();
        assert_eq!(word(&app), 1, "1 at load");
        for (line, want) in [
            ("cameraClip", 0),
            ("cameraClip", 1),
            ("cameraClip 5", 5),
            ("cameraClip", 0),
            ("CAMERACLIP 7x", 7),
            ("cameraClip -3", -3),
            ("cameraClip", 0),
            ("cameraClip abc", 0),
            ("cameraClip", 1),
        ] {
            assert!(execute(app.world_mut(), line).is_empty(), "{line}");
            assert_eq!(word(&app), want, "{line}");
        }
    }

    /// `0x50c3fb`: category 0 (`debug`), no help string.
    #[test]
    fn camera_clip_is_a_debug_command_with_no_help() {
        let mut app = camera_app();
        let debug = execute(app.world_mut(), "help debug");
        assert_eq!(debug[0], "Commands registered for the category debug:");
        assert!(
            debug[1..]
                .iter()
                .any(|l| l.split(", ").any(|n| n == "cameraClip")),
            "{debug:?}"
        );
        assert_eq!(
            execute(app.world_mut(), "help cameraclip"),
            [
                "Help for command cameraclip:",
                "     cameraclip No help yet"
            ]
        );
    }
}
