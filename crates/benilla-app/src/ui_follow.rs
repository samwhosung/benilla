//! The follow bridge: `FollowUnit`/`FollowByName` arm the follow and signal `AUTOFOLLOW_BEGIN`
//! inside their calls ([`crate::script_calls::ScriptCallHost`]), and the engine's own transitions
//! of [`crate::player::FollowState`] (a stop, the chat layer's `/follow`) fire
//! `AUTOFOLLOW_BEGIN`/`AUTOFOLLOW_END` here: the reference's whole follow interface to the UI
//! (there is no `IsFollowing()`).
//!
//! Deviation: every stop fires `END`, because the reference's three silent stops are a bug: two
//! writers reset the follow mode cell `0xc4d888` directly (`0x5fb646`, `0x60396f`), leaving a
//! stale guid at `0xc4d980`, and the canceller's early-out (`0x60fb70`) fires nothing.

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use crate::player::FollowState;
use crate::ui_script::UiFeed;

/// The followee guid each VM was last told of, so the events fire on transitions only. A
/// `/reload` ends a follow before the VM goes ([`crate::player::cancel_move_action_on_ui_shutdown`]),
/// so the next VM starts with none.
#[derive(Resource, Default)]
pub(crate) struct FollowFeed {
    vm: crate::ui_script::VmMemo<FollowFeedMemo>,
}

#[derive(Default)]
struct FollowFeedMemo {
    told: Option<u64>,
}

impl FollowFeed {
    /// A follow of `guid` armed inside a call, which signalled it there: the followee `script`
    /// was told of before, now replaced by `guid`.
    pub(crate) fn tell(&mut self, script: &UiScript, guid: u64) -> Option<u64> {
        self.vm.get(script).told.replace(guid)
    }
}

fn feed_follow(
    mut vm: crate::script_calls::VmWith<(Res<'static, FollowState>, ResMut<'static, FollowFeed>)>,
) {
    vm.feed(|script, (follow, mut feed), fires| {
        let Some(script) = script else {
            return;
        };
        let memo = feed.vm.get(script);
        match (follow.guid, memo.told) {
            // Started, or switched subject: a switch fires END first, as the reference's arm
            // routine (`0x611130`) calls the canceller (`0x61113e`) before arming.
            (Some(guid), told) if told != Some(guid) => {
                if told.is_some() {
                    fires.post("AUTOFOLLOW_END", vec![]);
                }
                fires.post(
                    "AUTOFOLLOW_BEGIN",
                    vec![ScriptValue::Str(follow.name.clone())],
                );
                memo.told = Some(guid);
            }
            // Stopped. END carries no name: `AutoFollowStatus` (`ZoneText.lua`) reuses BEGIN's.
            (None, Some(_)) => {
                fires.post("AUTOFOLLOW_END", vec![]);
                memo.told = None;
            }
            _ => {}
        }
    })
}

pub(crate) struct UiFollowPlugin;

impl Plugin for UiFollowPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FollowFeed>()
            .add_systems(Update, feed_follow.in_set(UiFeed));
    }
}
