//! The world's console commands: the thirteen `CWorld::Initialize` (`0x66f6c0`) registers
//! through `ConsoleCommandRegister` (`0x66fa1d`-`0x66faf5`), each in category 1 with no help, and
//! `InitSky`'s `SkySunGlare` (`0x6d1d4d`). `/console` and `ConsoleExec` (`0x515f10`) reach them
//! through `0x63ce00`, which has no category gate. Each handler writes the world's flags
//! ([`WorldFlags`]), the ripple gate ([`WaterRipples`]) or a level no pass reads yet
//! ([`WorldLevels`]), and prints what its own `ConsoleWrite` does.

use benilla_ui::script::scanf_d;
use benilla_world::water_fx::WaterRipples;
use benilla_world::world_flags::WorldFlags;
use bevy::prelude::*;

use crate::console::{ConsoleCommandApp, ConsoleHandler, GRAPHICS};

/// The levels the world's console commands store besides the flags and the ripple gate, at their
/// start-up values. No world pass reads them yet, so they stay here until one does.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WorldLevels {
    /// `maxLOD` (`0x673200`): the near chunk level `[0x867964]`, 2 or 3, 3 at start-up
    /// (`0x66f8ae`).
    pub(crate) max_lod: u32,
    /// `waterMaxLOD` (`0x6736c0`): `[0x86a030]`, 0 to 4, initialised 4.
    pub(crate) water_max_lod: i32,
    /// `waterWaves` (`0x673710`): `[0x86a034]`, initialised 2.
    pub(crate) water_waves: i32,
    /// `waterSpecular` (`0x673730`): `[0x86a038]`, initialised 1.
    pub(crate) water_specular: i32,
    /// `showSimpleDoodads` (`0x673ac0`): `[0xc7b2c4]`, cleared at start-up (`0x66f714`).
    pub(crate) simple_doodads: bool,
    /// `setShadow` (`0x673290`): the shadow colour `0x672590` stores, packed first float in the
    /// top byte, which the world frame writes back from the light state each frame (`0x66fff3`).
    pub(crate) shadow_color: Option<u32>,
}

impl Default for WorldLevels {
    fn default() -> Self {
        Self {
            max_lod: 3,
            water_max_lod: 4,
            water_waves: 2,
            water_specular: 1,
            simple_doodads: false,
            shadow_color: None,
        }
    }
}

pub(crate) struct WorldConsolePlugin;

impl Plugin for WorldConsolePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldFlags>()
            .init_resource::<WorldLevels>()
            .init_resource::<WaterRipples>()
            .init_resource::<benilla_world::sun::SunGlare>();
        // `0x66f6c0`'s order.
        const WORLD: [(&str, ConsoleHandler); 13] = [
            ("showDetailDoodads", show_detail_doodads),
            ("maxLOD", max_lod),
            ("showCull", show_cull),
            ("setShadow", set_shadow),
            ("waterMaxLOD", water_max_lod),
            ("waterWaves", water_waves),
            ("waterSpecular", water_specular),
            ("waterRipples", water_ripples),
            ("waterParticulates", water_particulates),
            ("showShadow", show_shadow),
            ("showLowDetail", show_low_detail),
            ("showSimpleDoodads", show_simple_doodads),
            ("detailDoodadAlpha", detail_doodad_alpha),
        ];
        for (name, run) in WORLD {
            app.console_command(name, GRAPHICS, None, run);
        }
        // Registered by `InitSky` at the first map load (`0x6d1d43`-`0x6d1d4d`).
        app.console_command("SkySunGlare", GRAPHICS, Some("0, 1"), sky_sun_glare);
        // Every world entry loads the map, which re-runs `InitSky` and re-arms the glares; a
        // worldport's own load is `benilla_world::sun`'s.
        app.add_systems(
            OnEnter(crate::client_state::ClientState::InWorld),
            |mut glare: ResMut<benilla_world::sun::SunGlare>| glare.enabled = true,
        );
    }
}

/// A world-flag toggle (`0x673050` and its siblings): the bit set is cleared with the first line,
/// a clear one set with the second, each through `ConsoleWrite` in class 0.
fn toggle(world: &mut World, bit: u32, off: &str, on: &str) -> Vec<String> {
    let mut flags = world.get_resource_or_init::<WorldFlags>();
    let line = if flags.has(bit) {
        flags.0 &= !bit;
        off
    } else {
        flags.0 |= bit;
        on
    };
    vec![line.to_string()]
}

/// `0x673050`: bit 0x100000 (`0x867ac4`, `0x867aac`).
fn show_detail_doodads(world: &mut World, _args: &str) -> Vec<String> {
    toggle(
        world,
        WorldFlags::DETAIL_DOODADS,
        "Detail doodads disabled.",
        "Detail doodads enabled.",
    )
}

/// `0x673250`: bit 0x20 (`0x867c34`, `0x867c18`).
fn show_cull(world: &mut World, _args: &str) -> Vec<String> {
    toggle(
        world,
        WorldFlags::CULL,
        "Terrain culling disabled.",
        "Terrain culling enabled.",
    )
}

/// `0x673770`: bit 0x2000000 (`0x867ea0`, `0x867e88`).
fn water_particulates(world: &mut World, _args: &str) -> Vec<String> {
    toggle(
        world,
        WorldFlags::PARTICULATES,
        "Particulates disabled",
        "Particulates enabled",
    )
}

/// `0x673a30`: bit 0x40 (`0x867fe8`, `0x867fd0`).
fn show_shadow(world: &mut World, _args: &str) -> Vec<String> {
    toggle(
        world,
        WorldFlags::SHADOW,
        "Terrain shadow disabled.",
        "Terrain shadow enabled.",
    )
}

/// `0x673a70`: bit 0x4000000 (`0x868020`, `0x868004`).
fn show_low_detail(world: &mut World, _args: &str) -> Vec<String> {
    toggle(
        world,
        WorldFlags::LOW_DETAIL,
        "Terrain low detail disabled.",
        "Terrain low detail enabled.",
    )
}

/// `0x673ac0`: the simple-doodad word `[0xc7b2c4]`, set to 1 or 0 (`0x868058`, `0x868040`).
fn show_simple_doodads(world: &mut World, _args: &str) -> Vec<String> {
    let mut levels = world.get_resource_or_init::<WorldLevels>();
    levels.simple_doodads = !levels.simple_doodads;
    let line = if levels.simple_doodads {
        "Simple doodads enabled."
    } else {
        "Simple doodads disabled."
    };
    vec![line.to_string()]
}

/// The parse buffer's address `0xc4ee08`, which a handler's `push ecx` leaves in the slot its
/// `sscanf("%d")` (`0x835154`) did not fill.
const UNFILLED_SLOT: u32 = 0x00c4_ee08;

/// `0x673200`: the `%d` slot, unsigned: above 3 stores 3 (`0x67321e`), below 2 stores 2
/// (`0x673236`), into the near chunk level `[0x867964]`. Prints nothing.
fn max_lod(world: &mut World, args: &str) -> Vec<String> {
    let v = scanf_d(args).map_or(UNFILLED_SLOT, |v| v as u32);
    world.get_resource_or_init::<WorldLevels>().max_lod = v.clamp(2, 3);
    Vec::new()
}

/// `0x6736c0`: the `%d` straight into `[0x86a030]`, which no number leaves as it was, then held to
/// 0..=4, signed. Prints nothing.
fn water_max_lod(world: &mut World, args: &str) -> Vec<String> {
    let mut levels = world.get_resource_or_init::<WorldLevels>();
    if let Some(v) = scanf_d(args) {
        levels.water_max_lod = v;
    }
    levels.water_max_lod = levels.water_max_lod.clamp(0, 4);
    Vec::new()
}

/// A `%d` straight into a level, kept when no number parses, with no bound: `waterWaves`
/// (`0x673710`, `[0x86a034]`) and `waterSpecular` (`0x673730`, `[0x86a038]`). Prints nothing.
fn scan_level(world: &mut World, args: &str, level: fn(&mut WorldLevels) -> &mut i32) {
    if let Some(v) = scanf_d(args) {
        *level(&mut world.get_resource_or_init::<WorldLevels>()) = v;
    }
}

fn water_waves(world: &mut World, args: &str) -> Vec<String> {
    scan_level(world, args, |l| &mut l.water_waves);
    Vec::new()
}

fn water_specular(world: &mut World, args: &str) -> Vec<String> {
    scan_level(world, args, |l| &mut l.water_specular);
    Vec::new()
}

/// `0x673750`: the `%d` straight into `[0x86a03c]`, kept when no number parses, with no bound;
/// the ripple emitter's gate (`0x68bda3`). Prints nothing.
fn water_ripples(world: &mut World, args: &str) -> Vec<String> {
    if let Some(v) = scanf_d(args) {
        world.get_resource_or_init::<WaterRipples>().0 = v;
    }
    Vec::new()
}

/// `0x673290`: `sscanf("%f %f %f %f")` (`0x867c7c`), then each float is refused below 0.0
/// (`0x7ffd74`) or above 1.0 (`0x7ff9d8`) with "Color values must be in range (0.0,1.0)."
/// (`0x867c50`), a NaN passing both; else `__ftol(v · 255)` of each, the first in the top byte,
/// goes to the shadow colour setter `0x672590`. Prints nothing when it takes them.
///
/// Deviation: a float the scan leaves unread is 0.0, where the reference reads whatever its
/// uninitialised stack slot holds.
fn set_shadow(world: &mut World, args: &str) -> Vec<String> {
    let mut v = [0.0f32; 4];
    let mut rest = args;
    for slot in &mut v {
        let Some((f, used)) = benilla_ui::script::scanf_f_prefix(rest) else {
            break;
        };
        *slot = f;
        rest = &rest[used..];
    }
    if v.iter().any(|&f| f < 0.0 || f > 1.0) {
        return vec!["Color values must be in range (0.0,1.0).".to_string()];
    }
    let byte = |f: f32| benilla_ui::script::ftol(f64::from(f) * 255.0) as u8;
    let packed = v
        .iter()
        .fold(0u32, |acc, &f| (acc << 8) | u32::from(byte(f)));
    world.get_resource_or_init::<WorldLevels>().shadow_color = Some(packed);
    Vec::new()
}

/// `/console detailDoodadAlpha <n>`, the reference's console command (`0x6739a0`, registered by
/// `0x63f9e0` as a command, not a CVar, so it never persists): the ground-clutter cutout that
/// `texel.a x distance_ramp` is tested against (`[0xc7b2fc]`, 128 at `0x66f861`), so at the
/// default grass ends ~61 yd out of the 70 yd horizon. `sscanf(args, "%d")` (`0x835154`), then an
/// unsigned compare with 255 (`0x6739b9`): in range it is set without a word, above it (a
/// negative too) only "Alpha ref range 0 - 255." (`0x867f6c`) is printed. With no number the
/// slot keeps [`UNFILLED_SLOT`], which is above 255.
fn detail_doodad_alpha(world: &mut World, args: &str) -> Vec<String> {
    let alpha = scanf_d(args).map_or(UNFILLED_SLOT, |v| v as u32);
    if alpha > 0xff {
        return vec!["Alpha ref range 0 - 255.".to_string()];
    }
    if let Some(mut clutter) = world.get_resource_mut::<benilla_world::clutter::ClutterConfig>() {
        clutter.alpha_ref = alpha as f32 / 255.0;
    }
    Vec::new()
}

/// `SkySunGlare` (`0x6d1dc0`): `sscanf("%d")` into a slot `push ecx` seeded with the name pointer.
/// A nonzero answer and a nonzero slot set both glares' enabled word (`0xce97b0`, `0xce9700`) with
/// "SunGlare enabled.  Don't look directly at it." (`0x86f4c4`); anything else clears both with
/// "SunGlare disabled" (`0x86f4b0`). The runtime's `sscanf` answers `EOF` when the text ends
/// before a conversion, which passes the test (`0x6d1dd6`) with the slot unfilled, so an empty
/// or blank tail enables.
fn sky_sun_glare(world: &mut World, args: &str) -> Vec<String> {
    let blank = args
        .bytes()
        .all(|c| matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'));
    let enabled = blank || scanf_d(args).is_some_and(|v| v != 0);
    world
        .get_resource_or_init::<benilla_world::sun::SunGlare>()
        .enabled = enabled;
    let line = if enabled {
        "SunGlare enabled.  Don't look directly at it."
    } else {
        "SunGlare disabled"
    };
    vec![line.to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::{execute, ConsolePlugin};
    use crate::cvars::Cvars;
    use benilla_world::water_fx::WaterRipples;
    use benilla_world::world_flags::WorldFlags;

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
        run(&mut app, "waterRipples x");
        let l = levels(&app);
        assert_eq!(
            (
                l.water_waves,
                l.water_specular,
                app.world().resource::<WaterRipples>().0
            ),
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

    /// `0x6739a0`: `sscanf %d` and an unsigned compare with 255. In range it sets the cutout and
    /// says nothing; above it, a negative and text with no number print only the range line.
    #[test]
    fn detail_doodad_alpha_reads_like_the_references_command() {
        let mut world = World::new();
        world.insert_resource(benilla_world::clutter::ClutterConfig::default());
        let at = |w: &World| {
            (w.resource::<benilla_world::clutter::ClutterConfig>()
                .alpha_ref
                * 255.0)
                .round() as u32
        };
        for (args, said, now) in [
            ("12abc", None, 12),
            (" +12", None, 12),
            ("200", None, 200),
            ("300", Some("Alpha ref range 0 - 255."), 200),
            ("-1", Some("Alpha ref range 0 - 255."), 200),
            ("", Some("Alpha ref range 0 - 255."), 200),
            ("abc", Some("Alpha ref range 0 - 255."), 200),
        ] {
            let lines = detail_doodad_alpha(&mut world, args);
            assert_eq!(
                lines,
                said.map(String::from).into_iter().collect::<Vec<_>>(),
                "{args:?}"
            );
            assert_eq!(at(&world), now, "{args:?}");
        }
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
