//! The follow bridge: drains `FollowUnit`/`FollowByName` into [`crate::player::FollowRequest`]
//! and fires `AUTOFOLLOW_BEGIN`/`AUTOFOLLOW_END` on [`crate::player::FollowState`]'s transitions,
//! the reference's whole follow interface to the UI (there is no `IsFollowing()`).
//!
//! Deviation: every stop fires `END`, because the reference's three silent stops are a bug: two
//! writers reset the follow mode cell `0xc4d888` directly (`0x5fb646`, `0x60396f`), leaving a
//! stale guid at `0xc4d980`, and the canceller's early-out (`0x60fb70`) fires nothing.

use bevy::prelude::*;

use benilla_ui::script::{FollowRequest as ScriptFollowRequest, ScriptValue, UiScript};

use crate::player::{FollowRequest, FollowState};
use crate::ui_script::{UiFeed, UiInput};

/// The followee guid each VM was last told of, so the events fire on transitions only; a VM
/// replaced by `/reload` mid-follow starts at `None` and is told `BEGIN` again.
#[derive(Resource, Default)]
pub(crate) struct FollowFeed {
    vm: crate::ui_script::VmMemo<FollowFeedMemo>,
}

#[derive(Default)]
struct FollowFeedMemo {
    told: Option<u64>,
}

fn feed_follow(
    script: Option<NonSendMut<UiScript>>,
    follow: Res<FollowState>,
    mut feed: ResMut<FollowFeed>,
) {
    let Some(mut script) = script else {
        return;
    };
    let memo = feed.vm.get(&script);
    match (follow.guid, memo.told) {
        // Started, or switched subject: a switch fires END first, as the reference's arm routine
        // (`0x611130`) calls the canceller (`0x61113e`) before arming.
        (Some(guid), told) if told != Some(guid) => {
            if told.is_some() {
                script.fire_event("AUTOFOLLOW_END", vec![]);
            }
            script.fire_event(
                "AUTOFOLLOW_BEGIN",
                vec![ScriptValue::Str(follow.name.clone())],
            );
            memo.told = Some(guid);
        }
        // Stopped. END carries no name: `AutoFollowStatus` (`ZoneText.lua`) reuses BEGIN's.
        (None, Some(_)) => {
            script.fire_event("AUTOFOLLOW_END", vec![]);
            memo.told = None;
        }
        _ => {}
    }
}

fn drain_follow(script: Option<NonSendMut<UiScript>>, mut out: MessageWriter<FollowRequest>) {
    let Some(mut script) = script else {
        return;
    };
    for request in script.take_follow_requests() {
        out.write(match request {
            ScriptFollowRequest::ByUnit(unit) => FollowRequest::Unit(unit),
            ScriptFollowRequest::ByName { name, exact } => FollowRequest::Name { name, exact },
        });
    }
}

pub(crate) struct UiFollowPlugin;

impl Plugin for UiFollowPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FollowFeed>().add_systems(
            Update,
            (feed_follow.in_set(UiFeed), drain_follow.after(UiInput)),
        );
    }
}
