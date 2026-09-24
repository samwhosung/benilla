//! The frame ordering contract for the world-transition pipeline: a teleport's loading screen must
//! cover the swap the same frame, so four subsystems run in [`WorldStage`] order within `Update`.
//! Plugins join a stage with `.in_set(..)` and never name one another's functions.

use bevy::prelude::*;

/// The run condition every world-owning subsystem gates on: the world exists only while a
/// character is in it. The camera's gate (`player::setup::gate_world_camera`) is wider: it also
/// renders under an opaque loading screen, which compiles the world's pipelines early.
pub(crate) fn world_is_live(live: Res<WorldLive>) -> bool {
    live.0
}

/// Whether a world is live, written by whatever owns the session. Not a `States`: a mirrored state
/// applies at the next transition point, a frame late for every teardown.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub struct WorldLive(pub bool);

/// The falling edge of [`WorldLive`], the world's teardown trigger: the run condition only stops
/// upkeep, so without it a logout leaves the streamed world resident. Each gated system has its
/// own `Local`, so each sees the edge once.
pub(crate) fn world_left(live: Res<WorldLive>, mut was_live: Local<bool>) -> bool {
    let left = *was_live && !live.0;
    *was_live = live.0;
    left
}

/// Ordered stages of the per-frame world-transition pipeline (configured `.chain()`ed in `Update`).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorldStage {
    /// `net::apply_net_updates`: the world stream into entities, teleport and worldport surfaced
    /// before the player reads them.
    Net,
    /// `player::control`: input and the teleport or worldport snap.
    Input,
    /// `terrain_stream::stream_terrain`: tiles around the possibly just-snapped position, and the
    /// residency (`loading_screen::WorldLoadProgress`) the cover reads.
    Stream,
    /// `loading_screen::drive_loading_screen`: the cover while the surrounding tiles are not
    /// resident, set here and rendered the same frame.
    Present,
}

/// Installs the [`WorldStage`] order and the boot-order fix below; add it after `DefaultPlugins`,
/// whose `StatesPlugin` seeds the order the fix moves.
pub(crate) struct SchedulePlugin;

impl Plugin for SchedulePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldLive>();
        app.configure_sets(
            Update,
            (
                WorldStage::Net,
                WorldStage::Input,
                WorldStage::Stream,
                WorldStage::Present,
            )
                .chain(),
        );
        move_initial_state_transition_after_startup(app);
    }
}

/// Run the initial state's `OnEnter` after startup. `bevy_state` seeds the startup
/// `StateTransition` before `PreStartup` (`bevy_state-0.18.1/src/app.rs:309`), so an `OnEnter`
/// for the boot state would run before the patch chain, the Lua VM, the mixer and every catalog;
/// this moves it after `PostStartup`. Safe while no `Startup` system writes `NextState`;
/// `insert_state`/`init_state` insert `State<S>` at plugin build either way.
fn move_initial_state_transition_after_startup(app: &mut App) {
    use bevy::app::MainScheduleOrder;
    use bevy::state::state::StateTransition;

    let mut order = app.world_mut().resource_mut::<MainScheduleOrder>();
    let before = order.startup_labels.len();
    order.startup_labels.retain(|l| !(**l).eq(&StateTransition));
    // Re-seat only what was there: if `bevy_state` stops seeding it, do not invent it.
    if order.startup_labels.len() < before {
        order.insert_startup_after(PostStartup, StateTransition);
    }
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;
    use bevy::state::app::StatesPlugin;

    use super::SchedulePlugin;

    /// A stand-in for the composing binary's states.
    #[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    enum Screen {
        #[default]
        InWorld,
    }

    #[derive(Resource)]
    struct BuiltAtStartup;

    #[test]
    fn the_initial_states_on_enter_sees_what_startup_built() {
        #[derive(Resource, Default)]
        struct Saw(Option<bool>);

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin))
            .add_plugins(SchedulePlugin)
            .insert_state(Screen::InWorld)
            .init_resource::<Saw>()
            .add_systems(Startup, |mut c: Commands| c.insert_resource(BuiltAtStartup))
            .add_systems(
                OnEnter(Screen::InWorld),
                |built: Option<Res<BuiltAtStartup>>, mut saw: ResMut<Saw>| {
                    saw.0 = Some(built.is_some());
                },
            );

        app.update();

        assert_eq!(
            app.world().resource::<Saw>().0,
            Some(true),
            "the initial state's OnEnter ran before Startup — the boot-order trap is back"
        );
    }
}
