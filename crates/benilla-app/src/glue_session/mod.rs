//! What the glue screens stand on, whichever set is up, the stock GlueXML screens
//! ([`crate::ui_script::glue_mgr`]) or the native ones: the session's state, its network asks and
//! the run's own policy, never a screen's look or input. Nothing here is gated on which screens
//! stand.

pub(crate) mod edges;
pub(crate) mod login;
pub(crate) mod logout_smoke;
pub(crate) mod queue;
pub(crate) mod realm;
pub(crate) mod realm_smoke;
mod reconnect;
pub(crate) mod roster;
mod roundtrip;

use bevy::prelude::*;

/// The glue session's shared half, registered before any glue screen's plugin.
pub(crate) struct GlueSessionPlugin;

impl Plugin for GlueSessionPlugin {
    fn build(&self, app: &mut App) {
        realm::build(app);
        login::build(app);
        app.init_resource::<roster::Roster>()
            .init_resource::<roster::RunPick>()
            .add_systems(
                Update,
                edges::session_edges()
                    .in_set(edges::SessionEdges)
                    .after(benilla_world::schedule::WorldStage::Net),
            )
            .add_systems(
                Update,
                reconnect::answer_reconnect
                    .run_if(crate::ui_script::glue::stock_glue)
                    .after(edges::SessionEdges),
            )
            // After the UI tick, the glue manager's frame and the session's edges, in one chain:
            // on the stock screens each reads the frames those moved and presses their verbs.
            .add_systems(
                Update,
                (
                    // The run's own pick, which the edges arm and the manager's roster load lets
                    // the stock screen take.
                    roster::enter_run_pick,
                    logout_smoke::debug_logout_smoke.after(crate::player::ControlRestore),
                    roundtrip::debug_glue_roundtrip.run_if(roundtrip::enabled),
                    realm_smoke::debug_realm_smoke.run_if(realm_smoke::enabled),
                )
                    .chain()
                    .after(crate::ui_script::UiInput)
                    .after(crate::ui_script::glue_mgr::drive_glue_mgr)
                    .after(edges::SessionEdges)
                    .after(login::LoginPolicy),
            );
    }
}
