//! What the glue screens stand on, whichever set is up, the stock GlueXML screens
//! ([`crate::ui_script::glue_mgr`]) or the native ones: the session's state, its network asks and
//! the run's own policy, never a screen's look or input. Nothing here is gated on which screens
//! stand.

pub(crate) mod edges;
pub(crate) mod logout_smoke;
pub(crate) mod realm;
pub(crate) mod roster;

use bevy::prelude::*;

/// The glue session's shared half, registered before any glue screen's plugin.
pub(crate) struct GlueSessionPlugin;

impl Plugin for GlueSessionPlugin {
    fn build(&self, app: &mut App) {
        realm::build(app);
        app.init_resource::<roster::Roster>()
            .add_systems(
                Update,
                edges::session_edges()
                    .in_set(edges::SessionEdges)
                    .after(benilla_world::schedule::WorldStage::Net),
            )
            .add_systems(
                Update,
                logout_smoke::debug_logout_smoke
                    .after(crate::player::ControlRestore)
                    // After the UI tick: on the stock screens it reads their frames and presses
                    // their Enter World.
                    .after(crate::ui_script::UiInput),
            );
    }
}
