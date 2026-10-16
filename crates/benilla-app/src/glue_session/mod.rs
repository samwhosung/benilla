//! What the glue screens stand on, whichever set is up, the stock GlueXML screens
//! ([`crate::ui_script::glue_mgr`]) or the native ones: the session's state, its network asks and
//! the run's own policy, never a screen's look or input. Nothing here is gated on which screens
//! stand.

pub(crate) mod realm;

use bevy::prelude::*;

/// The glue session's shared half, registered before any glue screen's plugin.
pub(crate) struct GlueSessionPlugin;

impl Plugin for GlueSessionPlugin {
    fn build(&self, app: &mut App) {
        realm::build(app);
    }
}
