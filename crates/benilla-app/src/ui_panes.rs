//! The character panes' feed: the VM's `PlayerModel`, `DressUpModel` and `TabardModel` panes as
//! model values for their booths ([`crate::portrait`]), and the booths' answers back into the VM.
//!
//! A pane asks for a copy of its subject (`SetUnit`, `RefreshUnit`, `SetPetStablePaperdoll`, the
//! show hook) and arms sequences on it; the booth builds the copy and plays the arms. Each frame,
//! after the paint pass armed this frame's turns, the answers the booths gave last frame go in
//! (the copy built, `0x5059a0`; no unit to build from, `0x505b8d`; a natural end, `0x5062a0`) and
//! the panes come out.

use bevy::prelude::*;

use benilla_ui::script::CharacterPaneFrame;
use benilla_ui::widget::FrameHandle;

/// Every visible character pane, as the VM listed it this frame, and the VM it belongs to: a new
/// VM's arena reissues the same handles.
#[derive(Resource, Default)]
pub(crate) struct CharacterPanes {
    pub(crate) session: u64,
    pub(crate) rows: Vec<CharacterPaneFrame>,
}

/// The booths' answers, held for the next feed.
#[derive(Resource, Default)]
pub(crate) struct PaneAnswers {
    /// The VM the answered rows came from.
    pub(crate) session: u64,
    /// `(pane, request)`: the copy for that build request stands in the pane's booth.
    pub(crate) built: Vec<(FrameHandle, u64)>,
    /// `(pane, request)`: the client held no unit for that request's GUID.
    pub(crate) missed: Vec<(FrameHandle, u64)>,
    /// `(pane, build, serial)`: that arm on that copy ran to its natural end.
    pub(crate) finished: Vec<(FrameHandle, u64, u64)>,
}

/// Hand the VM the booths' answers, then list its panes.
pub(crate) fn feed_character_panes(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut panes: ResMut<CharacterPanes>,
    mut answers: ResMut<PaneAnswers>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            panes.session = 0;
            panes.rows.clear();
            answers.built.clear();
            answers.missed.clear();
            answers.finished.clear();
            return;
        };
        let session = script.session();
        let answers = &mut *answers;
        // An answer for another VM's rows names a handle this one may have reissued.
        let current = answers.session == session;
        for (h, build) in answers.built.drain(..) {
            if current {
                script.character_copy_built(h, build);
            }
        }
        for (h, build) in answers.missed.drain(..) {
            if current {
                script.character_copy_missed(h, build);
            }
        }
        for (h, build, serial) in answers.finished.drain(..) {
            if current {
                script.character_arm_finished(h, build, serial);
            }
        }
        panes.session = session;
        panes.rows = script.character_panes();
    })
}
