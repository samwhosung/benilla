//! The selection's object, as the object manager holds it this frame. The selection is a guid
//! (`[0xb4e2d8]`) and the client reads its object by that guid wherever it needs one, so
//! [`Selection::target`] is kept on whatever object the guid names now, and the selection ends
//! where the client's teardown `0x493910` ends it.

use bevy::prelude::*;

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
    mut vm: crate::script_calls::VmWith<Follow<'static, 'static>>,
) {
    let select = follow(&mut vm.own());
    // The teardown signals inside it ([`crate::script_calls::ScriptCallHost::engine_select`]).
    if let Some(select) = select {
        vm.engine_select(select, |mut f, owed| {
            owed.pay(&mut f.selection, &mut f.seam)
        });
    }
}

/// What following the selection's object reads and writes.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct Follow<'w, 's> {
    selection: ResMut<'w, Selection>,
    index: Res<'w, GuidIndex>,
    group: Res<'w, crate::ui_party::GroupState>,
    stores: Query<'w, 's, &'static ObjectStore>,
    self_guid: Res<'w, SelfGuid>,
    self_store: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    seam: crate::creature_anim::AttackSeam<'w, 's>,
}

/// [`follow_selection_object`]'s step: the teardown it ran, owing its signals.
fn follow(f: &mut Follow) -> Option<crate::script_calls::EngineSelect> {
    let Follow {
        selection,
        index,
        group,
        stores,
        self_guid,
        self_store,
        seam,
    } = f;
    let guid = selection.guid?;
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
        Some(held) if now == Some(held) => return None,
        // Its object deactivated, though the guid may name a fresh one already.
        Some(_) => !member(),
        // No object to deactivate.
        None => false,
    };
    if ends {
        let before = super::click::Standing::of(selection, seam);
        let send = super::click::clear_owing(selection, seam);
        return Some(before.owing(super::scan::Owed { send, swing: None }));
    }
    if selection.target != now {
        // The lit object went with its deactivation, and one that streams in under the guid is
        // not lit: only `SetSelection` lights (`0x493701`).
        selection.lit = None;
        selection.target = now;
    }
    None
}
