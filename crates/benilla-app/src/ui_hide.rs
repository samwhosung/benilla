//! **TOGGLEUI** — the hide-the-interface binding (`ALT-Z`).
//!
//! The state ([`UiHidden`]) and the binding that flips it; the two things that *obey* it are the
//! quad pass's mesh rebuild (`ui_pass::rebuild_ui_mesh` draws nothing) and the UI's pointer feed
//! (`ui_script::input::feed_ui_input` stops hit-testing). Kept out of `ui_pass` deliberately: this
//! is a *game binding*, not part of the render substrate.

use bevy::prelude::*;

use crate::char_select::{ClientState, InWorldGated};
use crate::ui_script::UiInput;

/// Is the player UI hidden right now? (`ALT-Z` — [`toggle_ui_hidden`].)
///
/// Hidden means *"draw nothing"*, not *"stop producing"*: the widget arena keeps ticking and both
/// quad lanes keep filling, so open panels, tooltip state, cooldown sweeps and chat scrollback are
/// exactly where they were the moment the UI comes back — only the pass's mesh batches go away.
/// **Everything that lands in `ui_pass::UiQuads` goes dark together**: the FrameXML layer, the
/// minimap, the V-plates, chat bubbles and floating combat text — the world and nothing else,
/// which is the point of the binding. What stays up: the dev overlays (their own camera and their
/// own dev chords — an instrument, not the player's UI), the glue/loading screens (Bevy UI
/// nodes, not quads), and the cursor.
///
/// The UI also stops taking the **mouse** while hidden: an invisible action bar must not eat a
/// click or arm a tooltip. The keyboard feed stays live — a hidden UI is still the client you're
/// playing, and `ENTER`/`ESCAPE`/the bar keys keep working exactly as the reference's do.
#[derive(Resource, Default)]
pub(crate) struct UiHidden(pub bool);

/// The TOGGLEUI binding + its state.
pub(crate) struct UiHidePlugin;

impl Plugin for UiHidePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiHidden>()
            .add_systems(
                Update,
                // After `UiInput`, like every other gameplay key reader: an EditBox that consumed
                // this frame's keys has already published its capture flag by then.
                toggle_ui_hidden.after(UiInput).in_set(InWorldGated),
            )
            .add_systems(OnExit(ClientState::InWorld), show_ui);
    }
}

/// TOGGLEUI through the binding table (default `ALT-Z` — 0870's finding stands: the
/// install's `bindings-cache.wtf` says `CTRL-Z` on all three accounts, but they descend from one
/// rebound profile, and a saved-state file is evidence about a *player*, never about the client).
/// The exact-modifier law and the typing gate this site used to enforce by hand — the
/// `toggle_chord` alt-and-nothing-else check, the AppKit `keyUp` repeat hazard — now live once,
/// in the dispatch.
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

/// Leaving the world clears the hide — safety, not fidelity: the binding is `InWorld`-only, so a UI
/// left hidden at logout would come back invisible with no on-screen affordance to explain it.
fn show_ui(mut hidden: ResMut<UiHidden>) {
    hidden.0 = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The consumer half only: TOGGLEUI fired → the flag flips, both ways. The chord/typing laws
    /// this file used to test by hand (ALT-and-nothing-else, the capture gate, bare Z belonging
    /// to the sheath toggle) are the DISPATCH's now, tested table-wide in
    /// `crate::bindings::tests` against real key events.
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
