//! The app-side **follow bridge** — the seam between the auto-follow movement mode
//! ([`crate::player::follow`]) and the UI that has to say so.
//!
//! Two directions, one small module:
//!
//! - **In.** Drain the Era API's follow intents ([`benilla_ui::script::FollowRequest`], queued by
//!   `FollowUnit`/`FollowByName`) into the app's own [`crate::player::FollowRequest`] funnel, which
//!   [`crate::target::by_name`] resolves. The unit popup's **Follow** row is the shipped caller;
//!   the slash command reaches the same funnel directly, because benilla parses slash lines in Rust.
//! - **Out.** Fire `AUTOFOLLOW_BEGIN` / `AUTOFOLLOW_END` on the follow state's transitions. That
//!   pair is the *entire* interface the reference gives the UI for follow — there is no
//!   `IsFollowing()` and no unit token for the followee, so a status display is only possible at
//!   all because the events carry the name.
//!
//! ## What the events drive
//!
//! `AutoFollowStatus` (ZoneText.xml) — a `setAllPoints` frame over `UIParent` carrying one
//! centre-anchored 20px `GameFontNormal` string. On `AUTOFOLLOW_BEGIN` it latches `arg1`, sets alpha
//! to 1 and shows `AUTOFOLLOWSTART` ("Following %s."); on `AUTOFOLLOW_END` it swaps the text to
//! `AUTOFOLLOWSTOP` ("You stop following %s.") — reusing the *latched* name, which is why END needs
//! no argument of its own — and starts a linear 4-second alpha fade to hidden. It is not the
//! FadingFrame kit; `AutoFollowStatus_OnUpdate` rolls its own fade, with no fade-in and no hold.
//!
//! So the "You stop following X" line is the reason [`crate::player::FollowState`] latches the name
//! at start rather than re-reading it: by the time the follow ends, the followee may well be gone.
//!
//! ## Transitions
//!
//! Start → `BEGIN`. Stop → `END`. A follow that *switches* subject fires **`END` then `BEGIN`**:
//! the reference's arm routine calls the canceller first (`0x61113e`, inside `0x611130`), so
//! re-issuing `/follow` while already following really does emit both, in that order. On screen the
//! pair is indistinguishable from `BEGIN` alone — `AutoFollowStatus_OnEvent`'s BEGIN arm is a full
//! reset, clearing `fadeTime` and restoring alpha — but an addon listening for `AUTOFOLLOW_END`
//! would notice, so we emit what the reference emits.
//!
//! ## The three paths where the reference does NOT fire `END`
//!
//! Two writers set the follow mode cell `0xc4d888` back to `0xc` **directly** (`0x5fb646`,
//! `0x60396f`), bypassing the canceller entirely: no event fires, and `0xc4d980` is left holding a
//! stale guid. A third miss is the canceller's own early-out (`0x60fb70`), which skips the fire
//! *and* the state clear. The two direct writers' triggers are INFERRED to be zoning / world-leave
//! / logout.
//!
//! We do not reproduce this. It is a reference **bug** whose visible symptom is the status line
//! surviving a zone change with nobody to hide it — and our shape cannot reproduce it anyway, since
//! the line follows [`FollowState`] rather than a separately-cleared cell. Recorded because "the
//! status text is stuck after a zone" is a real report someone may one day file *against the real
//! client*, and the answer is "yes, it does that".

use bevy::prelude::*;

use benilla_ui::script::{FollowRequest as ScriptFollowRequest, ScriptValue, UiScript};

use crate::player::{FollowRequest, FollowState};
use crate::ui_script::{UiFeed, UiInput};

/// What the VM has last been told, so the two events fire exactly on transitions and never once a
/// frame. Mirrors [`FollowState::guid`]; `None` means the VM believes no follow is running.
///
/// "What THIS VM has been told" is exactly what a [`crate::ui_script::VmMemo`] keys (1290): a
/// `/reload` (1291) replaces the VM mid-follow without touching [`FollowState`], and the fresh
/// memo's `None` makes the next frame re-fire `AUTOFOLLOW_BEGIN` — the replacement VM never heard
/// it, so its status line has to be told again.
#[derive(Resource, Default)]
pub(crate) struct FollowFeed {
    vm: crate::ui_script::VmMemo<FollowFeedMemo>,
}

/// The per-VM half of [`FollowFeed`] — the one transition base.
#[derive(Default)]
struct FollowFeedMemo {
    told: Option<u64>,
}

/// Push the follow state's transitions into the VM as the reference's two events.
fn feed_follow(
    script: Option<NonSendMut<UiScript>>,
    follow: Res<FollowState>,
    mut feed: ResMut<FollowFeed>,
) {
    let Some(mut script) = script else {
        return;
    };
    // Resolved against THIS VM (1290/1291): a replacement VM reads `told: None` and gets its BEGIN.
    let memo = feed.vm.get(&script);
    match (follow.guid, memo.told) {
        // Started, or switched subject. A switch emits END first, because the reference's arm
        // routine calls the canceller before arming (see the module header).
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
        // Stopped — by arrival at an unresolvable followee, by the turn-away cancel, or by any of
        // the input cancels. END carries no argument: the frame reuses the name BEGIN latched.
        (None, Some(_)) => {
            script.fire_event("AUTOFOLLOW_END", vec![]);
            memo.told = None;
        }
        _ => {}
    }
}

/// Drain the Era API's follow intents into the app's own funnel.
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
