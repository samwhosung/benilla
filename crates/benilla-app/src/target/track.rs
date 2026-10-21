//! The selection's object, as the object manager holds it this frame. The selection is a guid
//! (`[0xb4e2d8]`) and the client reads its object by that guid wherever it needs one, so
//! [`Selection::target`] is kept on whatever object the guid names now, and the selection ends
//! where the client's teardown `0x493910` ends it.

use bevy::prelude::*;

use crate::creature_anim::Engaged;
use crate::net::{GuidIndex, ObjectStore, SelfGuid, SelfPlayer};

use super::Selection;

/// Keep [`Selection::target`] on the selected guid's object, once the net stage has applied the
/// objects that stream in and out. A unit's deactivation (`0x5fbb60`), destroyed or out of range, runs
/// the teardown with its send on a matching selection (`0x5fbbe5`) unless `IsRaidMemberOrPet`
/// (`0x491900`, `0x5fbbcd`) holds the guid, whose selection stays as the guid alone; an object the
/// guid names again is the one every reader takes (the target circle compares guids each frame,
/// `0x614a96`). A guid with no object has no deactivation: only an explicit teardown ends it, the
/// roster's among them (`crate::ui_party::RosterRelease`), and the deselects.
pub(crate) fn follow_selection_object(
    mut selection: ResMut<Selection>,
    index: Res<GuidIndex>,
    group: Res<crate::ui_party::GroupState>,
    stores: Query<&ObjectStore>,
    self_guid: Res<SelfGuid>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    mut seam: crate::creature_anim::AttackSeam,
    engaged: Query<(), (With<Engaged>, With<SelfPlayer>)>,
) {
    let Some(guid) = selection.guid else {
        return;
    };
    let now = index.0.get(&guid).copied();
    let member = || {
        let held = |g: u64| {
            index
                .0
                .get(&g)
                .and_then(|e| stores.get(*e).ok())
                .map(|s| &s.0)
        };
        let me = self_guid
            .0
            .map(|g| (g, self_store.iter().next().map(|s| &s.0)));
        group.member_or_pet(guid, me, held)
    };
    let ends = match selection.target {
        // Still the object the guid names.
        Some(held) if now == Some(held) => return,
        // Its object deactivated, though the guid may name a fresh one already.
        Some(_) => !member(),
        // No object to deactivate.
        None => false,
    };
    if ends {
        super::click::clear(&mut selection, &mut seam, !engaged.is_empty());
    } else if selection.target != now {
        // The lit object went with its deactivation, and one that streams in under the guid is
        // not lit: only `SetSelection` lights (`0x493701`).
        selection.lit = None;
        selection.target = now;
    }
}
