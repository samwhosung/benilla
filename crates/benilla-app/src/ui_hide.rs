//! TOGGLEUI, the hide-the-interface binding (`ALT-Z`), and its state: while it is set the quad
//! pass draws nothing and the UI's pointer feed stops hit-testing.

use bevy::prelude::*;

use crate::char_select::{ClientState, InWorldGated};
use crate::ui_script::UiInput;

/// Whether the player UI is hidden: nothing in `ui_pass::UiQuads` is drawn and the UI takes no
/// mouse, while widgets keep ticking. The keyboard stays live, as in the reference. Open windows
/// stay open, where the stock binding runs `CloseAllWindows()` first (`Bindings.xml:655-661`).
#[derive(Resource, Default)]
pub(crate) struct UiHidden(pub bool);

pub(crate) struct UiHidePlugin;

impl Plugin for UiHidePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiHidden>()
            .add_systems(
                Update,
                // After `UiInput`: an EditBox that took this frame's keys has published its
                // capture flag by then.
                toggle_ui_hidden.after(UiInput).in_set(InWorldGated),
            )
            .add_systems(OnExit(ClientState::InWorld), show_ui);
    }
}

fn toggle_ui_hidden(binds: Res<crate::bindings::BindingsState>, mut hidden: ResMut<UiHidden>) {
    if !binds.fired(crate::bindings::cmd::TOGGLE_UI) {
        return;
    }
    hidden.0 = !hidden.0;
    info!(
        "ui: {} (TOGGLEUI)",
        if hidden.0 { "HIDDEN" } else { "shown" }
    );
}

/// Leaving the world clears the hide: the binding is in-world only, so a UI hidden at logout would
/// come back invisible with no way to see why.
fn show_ui(mut hidden: ResMut<UiHidden>) {
    hidden.0 = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fired_toggleui_flips_the_flag_both_ways() {
        let mut app = App::new();
        app.add_systems(Update, toggle_ui_hidden)
            .init_resource::<UiHidden>()
            .insert_resource(crate::bindings::BindingsState::test_fired(&[
                crate::bindings::cmd::TOGGLE_UI,
            ]));
        app.update();
        assert!(app.world().resource::<UiHidden>().0, "fired → hidden");
        app.update();
        assert!(!app.world().resource::<UiHidden>().0, "fired again → shown");
        app.world_mut()
            .insert_resource(crate::bindings::BindingsState::default());
        app.update();
        assert!(!app.world().resource::<UiHidden>().0, "no fire → no change");
    }
}
