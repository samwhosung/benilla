//! The chat bubbles' feed: this frame's live bubbles, gated, faded and seated by
//! [`crate::chat_bubble`], handed to the widget layer, which keeps them as `WorldFrame` children.

use bevy::prelude::*;

use crate::chat_bubble::BubbleFeed;

/// Every frame, after the bubble drive and before the script paint: write the live set into the
/// VM's bubble pool, which hides and pools the frames of lines that ended.
pub(crate) fn feed_bubbles(
    feed: Res<BubbleFeed>,
    // `None` in a run with no UI VM.
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
) {
    let (Some(mut script), Some(geometry)) = (script, feed.geometry) else {
        return;
    };
    script.sync_chat_bubbles(geometry, &feed.states);
}
