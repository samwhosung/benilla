//! The engine verbs inside their calls: the follow's start (`0x60fed0`), whose arm signals where
//! it runs.

use benilla_ui::script::{FollowRequest, ScriptValue, UiScript};

use super::ScriptCallHost;
use crate::target::FollowStart;
use crate::ui_cast::ReportMark;

impl ScriptCallHost<'_, '_> {
    /// `FollowUnit(unit)` (`0x489e00`) or `FollowByName(name, exact)` (`0x489ec0`), then
    /// `StartAutoMoveToObject(guid, 3)` (`0x60fed0`): the gate's line shown in place
    /// (`0x496720`), or the arm, whose stop of the auto-move it replaces signals `AUTOFOLLOW_END`
    /// for a follow (`0x61113e` → `0x60fb84`), then `AUTOFOLLOW_BEGIN` with the followee's name
    /// (`0x610071`), each running its listeners where it fires.
    pub(super) fn follow(
        &mut self,
        script: &mut UiScript,
        mark: &mut ReportMark,
        request: FollowRequest,
    ) {
        let request = match request {
            FollowRequest::ByUnit(token) => crate::player::FollowRequest::Unit(token),
            FollowRequest::ByName { name, exact } => {
                crate::player::FollowRequest::Name { name, exact }
            }
        };
        let name = match self.appliers.follow().start(&request) {
            FollowStart::Refused(key) => return self.appliers.chat().display_error(script, key),
            FollowStart::Silent => return,
            FollowStart::Started { name } => name,
        };
        let guid = self.appliers.follow().followee();
        let told = match self.appliers.follow_feed().as_deref_mut() {
            Some(feed) => feed.tell(script, guid),
            None => None,
        };
        // Deviation: the feed's stops fire `END` for every follow ([`crate::ui_follow`]), so the
        // arm's stop fires it for the follow this VM was last told of.
        if told.is_some() {
            script.fire_event("AUTOFOLLOW_END", vec![]);
            self.at_point(script, mark);
        }
        script.fire_event("AUTOFOLLOW_BEGIN", vec![ScriptValue::Str(name)]);
        self.at_point(script, mark);
    }
}
