//! The app's lifecycle, which every system may read: the screen that owns the session
//! ([`ClientState`]), the one "only while in the world" gate ([`InWorldGated`]),
//! and the engine's one-bit mirror of it.

use bevy::prelude::*;

/// The app's lifecycle: which screen owns the session.
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) enum ClientState {
    /// Parked pre-logon at the login screen, the IO thread waiting for credentials. The realm list
    /// is no state: `GlueParent.lua`'s `GlueScreenInfo` omits it, a `DIALOG` frame over any
    /// screen.
    #[default]
    Login,
    /// Parked at character select, the IO thread waiting for a pick; the create screen stands in
    /// it too (the IO thread serves create and delete in place).
    CharSelect,
    /// A character is in (or entering) the world.
    InWorld,
}

/// The one "only while in the world" gate; a member's own `run_if` ANDs with it. Configured for
/// `Update` only: a set's conditions are per schedule, so a member elsewhere needs the set
/// configured there too.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct InWorldGated;

/// The lifecycle state, its gate and its mirror.
pub(crate) struct ClientStatePlugin {
    /// The screen this session opens on: `InWorld` for a world capture, else `Login`.
    pub(crate) start: ClientState,
}

impl Plugin for ClientStatePlugin {
    fn build(&self, app: &mut App) {
        app.insert_state(self.start)
            .configure_sets(Update, InWorldGated.run_if(in_state(ClientState::InWorld)))
            // Ahead of every world stage, so `WorldLive` and its falling edge are this frame's.
            .add_systems(
                Update,
                publish_world_live.before(benilla_world::schedule::WorldStage::Net),
            );
    }
}

/// Mirror the session's screen onto the engine's one-bit "is there a world" fact, in one place so
/// no transition can forget it.
fn publish_world_live(
    state: Res<State<ClientState>>,
    mut live: ResMut<benilla_world::schedule::WorldLive>,
) {
    let now = benilla_world::schedule::WorldLive(*state.get() == ClientState::InWorld);
    if *live != now {
        *live = now;
    }
}
