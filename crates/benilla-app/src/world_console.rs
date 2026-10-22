//! The world's console commands: the thirteen `CWorld::Initialize` (`0x66f6c0`) registers.

use bevy::prelude::*;

pub(crate) struct WorldConsolePlugin;

impl Plugin for WorldConsolePlugin {
    fn build(&self, _app: &mut App) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::{execute, ConsolePlugin};
    use crate::cvars::Cvars;
    use benilla_world::world_flags::{WorldFlags, WorldLevels};

    fn world_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Cvars>()
            .insert_resource(benilla_world::clutter::ClutterConfig::default())
            .add_plugins((ConsolePlugin, WorldConsolePlugin));
        app
    }

    fn run(app: &mut App, line: &str) -> Vec<String> {
        execute(app.world_mut(), line)
    }

    fn flags(app: &App) -> u32 {
        app.world().resource::<WorldFlags>().0
    }

    fn levels(app: &App) -> WorldLevels {
        *app.world().resource::<WorldLevels>()
    }

    /// The six toggles flip their bit and print the state they leave (`ConsoleWrite` class 0):
    /// the five world-flag bits start set (`0x7100b73`, `0x66f6cb`), the simple-doodad word clear
    /// (`0x66f714`). None reads its arguments.
    #[test]
    fn the_toggles_flip_their_bit_and_say_so() {
        let mut app = world_app();
        assert_eq!(flags(&app), 0x0710_0b73);
        for (name, bit, off, on) in [
            (
                "showDetailDoodads",
                0x0010_0000,
                "Detail doodads disabled.",
                "Detail doodads enabled.",
            ),
            (
                "showCull",
                0x20,
                "Terrain culling disabled.",
                "Terrain culling enabled.",
            ),
            (
                "waterParticulates",
                0x0200_0000,
                "Particulates disabled",
                "Particulates enabled",
            ),
            (
                "showShadow",
                0x40,
                "Terrain shadow disabled.",
                "Terrain shadow enabled.",
            ),
            (
                "showLowDetail",
                0x0400_0000,
                "Terrain low detail disabled.",
                "Terrain low detail enabled.",
            ),
        ] {
            let before = flags(&app);
            assert_eq!(run(&mut app, &format!("{name} 1")), [off], "{name}");
            assert_eq!(flags(&app), before & !bit, "{name}");
            assert_eq!(run(&mut app, &name.to_ascii_lowercase()), [on], "{name}");
            assert_eq!(flags(&app), before, "{name}");
        }
        assert_eq!(
            run(&mut app, "showSimpleDoodads 0"),
            ["Simple doodads enabled."]
        );
        assert!(levels(&app).simple_doodads);
        assert_eq!(
            run(&mut app, "showSimpleDoodads"),
            ["Simple doodads disabled."]
        );
        assert!(!levels(&app).simple_doodads);
    }

    /// `maxLOD` (`0x673200`) reads a `%d` into a slot `push ecx` seeded with the name pointer, so
    /// no number stores 3; unsigned, above 3 is 3 and below 2 is 2. `waterMaxLOD` (`0x6736c0`)
    /// scans straight into its word, kept when no number parses, then holds it to 0..4, signed;
    /// `waterWaves`, `waterSpecular` and `waterRipples` scan with no bound. None prints.
    #[test]
    fn the_levels_read_like_the_references_handlers() {
        let mut app = world_app();
        for (args, want) in [
            ("2", 2),
            ("1", 2),
            ("0", 2),
            ("3", 3),
            ("9", 3),
            ("2", 2),
            ("-1", 3),
            ("2", 2),
            ("", 3),
            ("2", 2),
            ("x", 3),
        ] {
            assert!(run(&mut app, &format!("maxLOD {args}")).is_empty());
            assert_eq!(levels(&app).max_lod, want, "maxLOD {args:?}");
        }
        for (args, want) in [
            ("9", 4),
            ("-3", 0),
            ("2", 2),
            ("", 2),
            ("abc", 2),
            (" 3x", 3),
        ] {
            assert!(run(&mut app, &format!("waterMaxLOD {args}")).is_empty());
            assert_eq!(levels(&app).water_max_lod, want, "waterMaxLOD {args:?}");
        }
        assert_eq!(levels(&app).water_waves, 2);
        run(&mut app, "waterWaves 7");
        run(&mut app, "waterWaves");
        run(&mut app, "waterSpecular -5");
        run(&mut app, "waterRipples 0");
        let l = levels(&app);
        assert_eq!(
            (l.water_waves, l.water_specular, l.water_ripples),
            (7, -5, 0)
        );
    }

    /// `setShadow` (`0x673290`) reads `%f %f %f %f`, refuses any outside 0 to 1 (a NaN passes)
    /// with the range line, and packs `ftol(v·255)` of each, the first in the top byte.
    #[test]
    fn set_shadow_packs_four_unit_floats() {
        let mut app = world_app();
        assert!(run(&mut app, "setShadow 0.5 0.25 1 0").is_empty());
        assert_eq!(levels(&app).shadow_color, Some(0x7f3f_ff00));
        for bad in ["2 0 0 0", "0 0 0 -0.1", "0.5 1.01 0 0"] {
            assert_eq!(
                run(&mut app, &format!("setShadow {bad}")),
                ["Color values must be in range (0.0,1.0)."],
                "{bad}"
            );
        }
        assert_eq!(levels(&app).shadow_color, Some(0x7f3f_ff00));
    }

    /// The thirteen register in category 1 with no help, in `0x66f6c0`'s order
    /// (`0x66fa1d`-`0x66faf5`).
    #[test]
    fn the_thirteen_list_under_graphics_in_order() {
        let mut app = world_app();
        let names: Vec<String> = run(&mut app, "help graphics")[1..]
            .iter()
            .flat_map(|l| l.split(", ").map(str::to_string).collect::<Vec<_>>())
            .collect();
        assert_eq!(
            names[..13],
            [
                "showDetailDoodads",
                "maxLOD",
                "showCull",
                "setShadow",
                "waterMaxLOD",
                "waterWaves",
                "waterSpecular",
                "waterRipples",
                "waterParticulates",
                "showShadow",
                "showLowDetail",
                "showSimpleDoodads",
                "detailDoodadAlpha",
            ]
        );
        assert_eq!(
            run(&mut app, "help maxlod"),
            ["Help for command maxlod:", "     maxlod No help yet"]
        );
    }

    /// `SkySunGlare` (`0x6d1dc0`, help `0, 1`, category 1): a `%d` that is nonzero sets both glares
    /// with the warning line, a zero or no number clears them. The slot is seeded with the name
    /// pointer (`push ecx`) and `sscanf` answers `EOF` for an empty tail, which the handler takes
    /// as a parse (`0x6d1dd6` tests only for 0), so a bare `SkySunGlare` enables.
    #[test]
    fn sky_sun_glare_sets_both_glares() {
        let mut app = world_app();
        let glare = |app: &App| {
            app.world()
                .resource::<benilla_world::sun::SunGlare>()
                .enabled
        };
        let on = "SunGlare enabled.  Don't look directly at it.";
        assert_eq!(run(&mut app, "SkySunGlare 0"), ["SunGlare disabled"]);
        assert!(!glare(&app));
        assert_eq!(run(&mut app, "skysunglare -2"), [on]);
        assert!(glare(&app));
        assert_eq!(run(&mut app, "SkySunGlare abc"), ["SunGlare disabled"]);
        assert_eq!(run(&mut app, "SkySunGlare"), [on]);
        assert_eq!(run(&mut app, "SkySunGlare   "), [on]);
        assert!(glare(&app));
        assert_eq!(
            run(&mut app, "help SkySunGlare"),
            ["Help for command SkySunGlare:", "     SkySunGlare 0, 1"]
        );
    }
}
